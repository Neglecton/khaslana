"""仅供离线集成测试：127.0.0.1 SSH 公钥认证和最小空仓库 ref 广告。

不执行 shell/git，不实现 pack；仅读取测试传入的公钥。主机私钥仅在内存中。
启动：python -u scripts/ssh-local-test-server.py --authorized-key TEMP.pub
stdin 接受 rotate/stop；stdout 是 ready/rotated JSON。--self-test 验证服务自身。
"""
import argparse
import base64
import io
import json
import logging
import socket
import threading

import paramiko
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ed25519

logging.getLogger("paramiko").setLevel(logging.CRITICAL)


def new_key():
    pem = ed25519.Ed25519PrivateKey.generate().private_bytes(
        serialization.Encoding.PEM,
        serialization.PrivateFormat.OpenSSH,
        serialization.NoEncryption(),
    )
    return paramiko.Ed25519Key.from_private_key(io.StringIO(pem.decode("ascii")))


class Server:
    def __init__(self, authorized, events=None):
        self.authorized = authorized
        self.events = events
        self.lock = threading.Lock()
        self.key = new_key()
        self.stopped = threading.Event()
        self.transports = []
        self.workers = []
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(8)
        self.listener.settimeout(0.2)
        self.port = self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.accept, daemon=True)
        self.thread.start()

    def event(self, name):
        if self.events:
            with self.lock:
                with open(self.events, "a", encoding="utf-8") as output:
                    output.write(json.dumps({"event": name}) + "\n")

    def info(self, event):
        return {"event": event, "port": self.port, "algorithm": self.key.get_name(),
                "raw_key": self.key.get_base64()}

    def rotate(self):
        self.key = new_key()
        return self.info("rotated")

    def accept(self):
        while not self.stopped.is_set():
            try:
                client, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            client.settimeout(8)
            worker = threading.Thread(target=self.serve, args=(client, self.key), daemon=True)
            self.workers.append(worker)
            worker.start()

    def serve(self, client, host_key):
        owner = self

        class Auth(paramiko.ServerInterface):
            def get_allowed_auths(self, username):
                return "publickey"

            def check_auth_publickey(self, username, key):
                if username == "git" and key.asbytes() == owner.authorized:
                    return paramiko.AUTH_SUCCESSFUL
                return paramiko.AUTH_FAILED

            def check_channel_request(self, kind, chanid):
                return paramiko.OPEN_SUCCEEDED if kind == "session" else paramiko.OPEN_FAILED_ADMINISTRATIVELY_PROHIBITED

            def check_channel_exec_request(self, channel, command):
                if command != b"git-upload-pack '/offline.git'":
                    return False
                # 收到认证后的 exec 才记成功，不能把无签名 publickey probe 当认证通过。
                owner.event("authenticated_exec")
                ready.set()
                return True

        transport = paramiko.Transport(client)
        transport.banner_timeout = 8
        transport.auth_timeout = 8
        self.transports.append(transport)
        ready = threading.Event()
        try:
            transport.add_server_key(host_key)
            transport.start_server(server=Auth())
            channel = transport.accept(8)
            if channel is None or not ready.wait(8):
                return
            # Git v0 空仓库的伪引用，仅用于 connect_auth 的 ref 握手。
            body = b"0" * 40 + b" capabilities^{}\x00multi_ack thin-pack side-band-64k ofs-delta\n"
            channel.sendall(f"{len(body) + 4:04x}".encode("ascii") + body + b"0000")
            while transport.is_active() and not self.stopped.wait(0.05):
                if channel.closed:
                    break
        except (EOFError, OSError, paramiko.SSHException):
            # 拒绝主机身份和错误口令的连接主动断开属于测试正常路径。
            pass
        finally:
            transport.close()

    def close(self):
        self.stopped.set()
        self.listener.close()
        self.thread.join(2)
        for transport in self.transports:
            transport.close()
        for worker in self.workers:
            worker.join(2)


def self_test():
    key = new_key()
    server = Server(key.asbytes())
    try:
        first = server.key.asbytes()
        for _ in range(2):
            with socket.create_connection(("127.0.0.1", server.port), timeout=5) as sock:
                with paramiko.Transport(sock) as transport:
                    transport.connect(username="git", pkey=key, hostkey=server.key)
                    assert transport.is_authenticated()
                    channel = transport.open_session(timeout=5)
                    channel.settimeout(5)
                    channel.exec_command("git-upload-pack '/offline.git'")
                    received = b""
                    while not received.endswith(b"0000"):
                        block = channel.recv(1024)
                        assert block, "ref 广告提前结束"
                        received += block
                    assert b"capabilities^{}\x00" in received
            server.rotate()
        assert first != server.key.asbytes()
        print("PASS: loopback Ed25519 signed auth, ref advertisement, host-key rotation", flush=True)
    finally:
        server.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--authorized-key")
    parser.add_argument("--events")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.authorized_key:
        parser.error("--authorized-key is required")
    with open(args.authorized_key, encoding="utf-8") as source:
        fields = source.read().split()
    if len(fields) < 2 or fields[0] != "ssh-ed25519":
        parser.error("only a test Ed25519 public key is accepted")
    server = Server(base64.b64decode(fields[1], validate=True), args.events)
    try:
        print(json.dumps(server.info("ready")), flush=True)
        import sys
        for line in sys.stdin:
            if line.strip() == "rotate":
                print(json.dumps(server.rotate()), flush=True)
            elif line.strip() == "stop":
                break
    finally:
        server.close()


if __name__ == "__main__":
    main()
