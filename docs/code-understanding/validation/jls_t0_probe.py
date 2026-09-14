#!/usr/bin/env python3
"""JLS-T0 real Eclipse JDT LS stdio probe; psutil is optional for RSS sampling."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import queue
import statistics
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Any
from urllib.parse import unquote, urlparse

try:
    import psutil  # type: ignore
except ImportError:
    psutil = None


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def directory_stats(path: Path) -> dict[str, int]:
    files = 0
    size = 0
    if path.exists():
        for item in path.rglob("*"):
            if item.is_file():
                files += 1
                size += item.stat().st_size
    return {"files": files, "bytes": size}


def project_snapshot(path: Path) -> dict[str, dict[str, Any]]:
    result: dict[str, dict[str, Any]] = {}
    for item in path.rglob("*"):
        if not item.is_file() or ".git" in item.parts:
            continue
        relative = item.relative_to(path).as_posix()
        result[relative] = {"bytes": item.stat().st_size, "sha256": sha256(item)}
    return result


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    if not ordered:
        return 0.0
    index = min(len(ordered) - 1, max(0, int((len(ordered) - 1) * fraction + 0.999999)))
    return ordered[index]


def uri_to_path(uri: str) -> Path | None:
    parsed = urlparse(uri)
    if parsed.scheme != "file":
        return None
    raw = unquote(parsed.path)
    if os.name == "nt" and raw.startswith("/") and len(raw) > 2 and raw[2] == ":":
        raw = raw[1:]
    return Path(raw)


def utf16_position(text: str, absolute_offset: int) -> dict[str, int]:
    prefix = text[:absolute_offset]
    line = prefix.count("\n")
    line_prefix = prefix.rsplit("\n", 1)[-1]
    character = len(line_prefix.encode("utf-16-le")) // 2
    return {"line": line, "character": character}


def anchor_position(project: Path, spec: dict[str, Any], prepare: bool = False) -> dict[str, int]:
    source = (project / spec["file"]).read_text(encoding="utf-8")
    anchor = (spec.get("prepare_anchor") or spec["anchor"]) if prepare else spec["anchor"]
    start = source.index(anchor)
    symbol = spec["symbol"]
    symbol_offset = source.index(symbol, start, start + len(anchor))
    return utf16_position(source, symbol_offset)


def location_uri(item: dict[str, Any]) -> str | None:
    return item.get("uri") or item.get("targetUri")


class LspClient:
    def __init__(self, command: list[str], stderr_path: Path):
        creationflags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        self.stderr_stream = stderr_path.open("wb")
        self.process = subprocess.Popen(
            command,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.stderr_stream,
            creationflags=creationflags,
        )
        self.memory_stop = threading.Event()
        self.memory_samples: list[dict[str, Any]] = []
        self.memory_reader = threading.Thread(target=self._sample_memory, name="jls-t0-memory", daemon=True)
        self.memory_reader.start()
        self.next_id = 1
        self.pending: dict[int, queue.Queue[dict[str, Any]]] = {}
        self.pending_lock = threading.Lock()
        self.write_lock = threading.Lock()
        self.notifications: list[dict[str, Any]] = []
        self.server_requests: list[str] = []
        self.active_progress: set[str] = set()
        self.ready_seen = False
        self.reader_error: str | None = None
        self.reader = threading.Thread(target=self._read_loop, name="jls-t0-lsp-reader", daemon=True)
        self.reader.start()

    def _sample_memory(self) -> None:
        if psutil is None:
            return
        try:
            root = psutil.Process(self.process.pid)
            while not self.memory_stop.wait(0.1):
                processes = [root] + root.children(recursive=True)
                rss = 0
                names: list[str] = []
                for process in processes:
                    try:
                        rss += process.memory_info().rss
                        names.append(process.name())
                    except (psutil.NoSuchProcess, psutil.AccessDenied):
                        pass
                self.memory_samples.append({"rss_bytes": rss, "processes": len(names), "names": sorted(set(names))})
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            return

    def memory_report(self) -> dict[str, Any]:
        if psutil is None:
            return {"available": False, "reason": "optional psutil module is not installed"}
        if not self.memory_samples:
            return {"available": False, "reason": "no process-tree sample collected"}
        peak = max(self.memory_samples, key=lambda item: item["rss_bytes"])
        return {
            "available": True,
            "sampler": f"psutil {psutil.__version__}",
            "sample_interval_ms": 100,
            "samples": len(self.memory_samples),
            "peak_rss_bytes": peak["rss_bytes"],
            "peak_process_count": peak["processes"],
            "peak_process_names": peak["names"],
        }

    def _write(self, message: dict[str, Any]) -> None:
        assert self.process.stdin is not None
        body = json.dumps(message, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        frame = f"Content-Length: {len(body)}\r\n\r\n".encode("ascii") + body
        with self.write_lock:
            self.process.stdin.write(frame)
            self.process.stdin.flush()

    def request_async(self, method: str, params: Any) -> int:
        with self.pending_lock:
            request_id = self.next_id
            self.next_id += 1
            self.pending[request_id] = queue.Queue(maxsize=1)
        self._write({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        return request_id

    def wait_response(self, request_id: int, timeout: float) -> Any:
        with self.pending_lock:
            response_queue = self.pending[request_id]
        try:
            response = response_queue.get(timeout=timeout)
        finally:
            with self.pending_lock:
                self.pending.pop(request_id, None)
        if "error" in response:
            raise RuntimeError(f"LSP error: {response['error']}")
        return response.get("result")

    def request(self, method: str, params: Any, timeout: float = 30.0) -> Any:
        return self.wait_response(self.request_async(method, params), timeout)

    def notify(self, method: str, params: Any) -> None:
        self._write({"jsonrpc": "2.0", "method": method, "params": params})

    def _read_loop(self) -> None:
        assert self.process.stdout is not None
        try:
            while True:
                headers: dict[str, str] = {}
                while True:
                    line = self.process.stdout.readline()
                    if not line:
                        return
                    if line in (b"\r\n", b"\n"):
                        break
                    key, value = line.decode("ascii").split(":", 1)
                    headers[key.lower()] = value.strip()
                length = int(headers["content-length"])
                body = self.process.stdout.read(length)
                if len(body) != length:
                    raise EOFError(f"expected {length} bytes, received {len(body)}")
                self._handle(json.loads(body.decode("utf-8")))
        except Exception:
            self.reader_error = traceback.format_exc()

    def _handle(self, message: dict[str, Any]) -> None:
        if "method" in message and "id" in message:
            self.server_requests.append(message["method"])
            result: Any = None
            if message["method"] == "workspace/configuration":
                items = message.get("params", {}).get("items", [])
                result = [{} for _ in items]
            elif message["method"] == "workspace/applyEdit":
                result = {"applied": False, "failureReason": "JLS-T0 probe is read-only"}
            self._write({"jsonrpc": "2.0", "id": message["id"], "result": result})
            return
        if "id" in message:
            with self.pending_lock:
                response_queue = self.pending.get(message["id"])
            if response_queue is not None:
                response_queue.put(message)
            return
        method = message.get("method", "")
        params = message.get("params")
        self.notifications.append({"method": method, "params": params})
        if method == "$/progress" and isinstance(params, dict):
            token = str(params.get("token"))
            kind = (params.get("value") or {}).get("kind")
            if kind == "begin":
                self.active_progress.add(token)
            elif kind == "end":
                self.active_progress.discard(token)
        if method == "language/status" and isinstance(params, dict):
            status_type = str(params.get("type", "")).lower()
            message_text = str(params.get("message", "")).lower()
            self.ready_seen = "ready" in status_type or "ready" in message_text

    def close(self) -> dict[str, Any]:
        shutdown_ok = False
        exit_code: int | None = None
        try:
            self.request("shutdown", None, timeout=10)
            shutdown_ok = True
            self.notify("exit", None)
            exit_code = self.process.wait(timeout=10)
        except Exception:
            self.process.terminate()
            try:
                exit_code = self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                exit_code = self.process.wait(timeout=5)
        self.memory_stop.set()
        self.memory_reader.join(timeout=2)
        self.stderr_stream.close()
        return {"shutdown_response": shutdown_ok, "exit_code": exit_code}


def locations(value: Any) -> list[dict[str, Any]]:
    if value is None:
        return []
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict)]
    if isinstance(value, dict):
        return [value]
    return []


def relative_locations(project: Path, items: list[dict[str, Any]]) -> list[str]:
    result: list[str] = []
    for item in items:
        uri = location_uri(item)
        path = uri_to_path(uri) if uri else None
        if path is None:
            result.append(uri or "<missing-uri>")
            continue
        try:
            result.append(path.resolve().relative_to(project.resolve()).as_posix())
        except ValueError:
            result.append(str(path))
    return sorted(set(result))


def prepare_call(client: LspClient, project: Path, spec: dict[str, Any]) -> dict[str, Any]:
    path = project / spec["file"]
    result = client.request(
        "textDocument/prepareCallHierarchy",
        {"textDocument": {"uri": path.resolve().as_uri()}, "position": anchor_position(project, spec, True)},
    )
    items = locations(result)
    if not items:
        raise RuntimeError(f"{spec['id']}: prepareCallHierarchy returned no item")
    return items[0]


def execute_query(client: LspClient, project: Path, spec: dict[str, Any]) -> tuple[Any, int]:
    path = project / spec["file"]
    text_document_position = {
        "textDocument": {"uri": path.resolve().as_uri()},
        "position": anchor_position(project, spec),
    }
    operation = spec["operation"]
    rpc_count = 1
    if operation == "definition":
        return client.request("textDocument/definition", text_document_position), rpc_count
    if operation == "implementation":
        return client.request("textDocument/implementation", text_document_position), rpc_count
    if operation == "references":
        params = dict(text_document_position)
        params["context"] = {"includeDeclaration": True}
        return client.request("textDocument/references", params), rpc_count
    if operation in ("incoming_calls", "outgoing_calls"):
        item = prepare_call(client, project, spec)
        rpc_count += 1
        method = "callHierarchy/incomingCalls" if operation == "incoming_calls" else "callHierarchy/outgoingCalls"
        return client.request(method, {"item": item}), rpc_count
    raise ValueError(f"unknown operation: {operation}")


def summarize_query(project: Path, spec: dict[str, Any], result: Any) -> dict[str, Any]:
    operation = spec["operation"]
    raw_items = locations(result)
    if operation == "incoming_calls":
        names = sorted({str(item.get("from", {}).get("name")) for item in raw_items})
        files = relative_locations(project, [item.get("from", {}) for item in raw_items])
    elif operation == "outgoing_calls":
        names = sorted({str(item.get("to", {}).get("name")) for item in raw_items})
        files = relative_locations(project, [item.get("to", {}) for item in raw_items])
    else:
        names = []
        files = relative_locations(project, raw_items)
    failures: list[str] = []
    for expected in spec.get("expect_files", []):
        if expected not in files:
            failures.append(f"missing file: {expected}")
    for rejected in spec.get("reject_files", []):
        if rejected in files:
            failures.append(f"unexpected file: {rejected}")
    for expected in spec.get("expect_names", []):
        if not any(name == expected or name.startswith(expected + "(") for name in names):
            failures.append(f"missing name: {expected}")
    if "expect_count" in spec and len(raw_items) != spec["expect_count"]:
        failures.append(f"expected count {spec['expect_count']}, got {len(raw_items)}")
    if len(raw_items) < spec.get("expect_min_count", 0):
        failures.append(f"expected at least {spec['expect_min_count']}, got {len(raw_items)}")
    return {
        "id": spec["id"],
        "operation": operation,
        "count": len(raw_items),
        "files": files,
        "names": names,
        "passed": not failures,
        "failures": failures,
    }


def wait_for_import(client: LspClient, timeout: float, symbol_probe: str) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    attempts = 0
    last_count = 0
    while time.monotonic() < deadline:
        attempts += 1
        try:
            result = client.request("workspace/symbol", {"query": symbol_probe}, timeout=10)
            last_count = len(locations(result))
            if last_count and not client.active_progress:
                return {"ready": True, "attempts": attempts, "workspace_symbols": last_count}
        except Exception:
            if client.process.poll() is not None:
                break
        time.sleep(1)
    return {
        "ready": False,
        "attempts": attempts,
        "workspace_symbols": last_count,
        "reader_error": client.reader_error,
        "process_exit": client.process.poll(),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--jdk", type=Path, required=True)
    parser.add_argument("--jdt", type=Path, required=True)
    parser.add_argument("--project", type=Path, required=True)
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--expectations", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--import-timeout", type=float, default=120.0)
    args = parser.parse_args()

    java = args.jdk / "bin" / ("java.exe" if os.name == "nt" else "java")
    javac = args.jdk / "bin" / ("javac.exe" if os.name == "nt" else "javac")
    launchers = sorted((args.jdt / "plugins").glob("org.eclipse.equinox.launcher_*.jar"))
    config_name = "config_win" if os.name == "nt" else "config_linux"
    config = args.jdt / config_name
    if not java.is_file() or not javac.is_file() or len(launchers) != 1 or not config.is_dir():
        parser.error("JDK/JDT layout is incomplete or launcher is ambiguous")

    project = args.project.resolve()
    workspace = args.workspace.resolve()
    if workspace.exists() and any(workspace.iterdir()):
        parser.error("--workspace must be absent or empty; the probe never deletes an existing workspace")
    workspace.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    stderr_path = args.output.with_suffix(".stderr.log")
    manifest = json.loads(args.expectations.read_text(encoding="utf-8"))
    expectations = manifest["queries"]
    symbol_probe = manifest.get("ready_symbol", "LoginApplicationService")
    before_project = project_snapshot(project)
    before_workspace = directory_stats(workspace)

    command = [
        str(java),
        "-Declipse.application=org.eclipse.jdt.ls.core.id1",
        "-Dosgi.bundles.defaultStartLevel=4",
        "-Declipse.product=org.eclipse.jdt.ls.core.product",
        "-Dlog.level=ALL",
        "-Xmx1G",
        "--add-modules=ALL-SYSTEM",
        "--add-opens", "java.base/java.util=ALL-UNNAMED",
        "--add-opens", "java.base/java.lang=ALL-UNNAMED",
        "-jar", str(launchers[0]),
        "-configuration", str(config),
        "-data", str(workspace),
    ]
    report: dict[str, Any] = {
        "schema": 1,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "platform": {"os": os.name, "sys_platform": sys.platform, "architecture": os.environ.get("PROCESSOR_ARCHITECTURE")},
        "paths": {"jdk": str(args.jdk.resolve()), "jdt": str(args.jdt.resolve()), "project": str(project), "workspace": str(workspace)},
        "artifacts": {"launcher": launchers[0].name, "launcher_sha256": sha256(launchers[0])},
        "command": command,
        "project_before": before_project,
        "workspace_before": before_workspace,
    }
    parent = args.jdt.resolve().parent
    archives = sorted(parent.glob("jdt-language-server-*.tar.gz"))
    jdk_archives = sorted(parent.glob("OpenJDK21U-jdk_*"))
    if archives:
        report["artifacts"]["jdt_archive"] = {"path": str(archives[-1]), "bytes": archives[-1].stat().st_size, "sha256": sha256(archives[-1])}
    if jdk_archives:
        report["artifacts"]["jdk_archive"] = {"path": str(jdk_archives[-1]), "bytes": jdk_archives[-1].stat().st_size, "sha256": sha256(jdk_archives[-1])}

    client = LspClient(command, stderr_path)
    started = time.perf_counter()
    try:
        settings = {
            "java": {
                "autobuild": {"enabled": False},
                "import": {"maven": {"enabled": True}, "gradle": {"enabled": True}},
                "configuration": {"runtimes": [{"name": "JavaSE-21", "path": str(args.jdk.resolve()), "default": True}]},
            }
        }
        initialize_started = time.perf_counter()
        initialize = client.request(
            "initialize",
            {
                "processId": os.getpid(),
                "rootUri": project.as_uri(),
                "workspaceFolders": [{"uri": project.as_uri(), "name": project.name}],
                "capabilities": {
                    "workspace": {"configuration": True, "workspaceFolders": True},
                    "textDocument": {
                        "definition": {"linkSupport": True},
                        "implementation": {"linkSupport": True},
                        "declaration": {"linkSupport": True},
                        "callHierarchy": {},
                    },
                    "window": {"workDoneProgress": True},
                },
                "initializationOptions": {
                    "settings": settings,
                    "extendedClientCapabilities": {"progressReportProvider": True, "classFileContentsSupport": True},
                },
                "clientInfo": {"name": "khaslana-jls-t0-probe", "version": "1"},
            },
            timeout=30,
        )
        report["initialize_ms"] = round((time.perf_counter() - initialize_started) * 1000, 3)
        report["server_capabilities"] = initialize.get("capabilities", {}) if isinstance(initialize, dict) else initialize
        client.notify("initialized", {})
        client.notify("workspace/didChangeConfiguration", {"settings": settings})
        for source_path in project.rglob("*.java"):
            client.notify(
                "textDocument/didOpen",
                {
                    "textDocument": {
                        "uri": source_path.resolve().as_uri(),
                        "languageId": "java",
                        "version": 1,
                        "text": source_path.read_text(encoding="utf-8"),
                    }
                },
            )
        import_started = time.perf_counter()
        report["import"] = wait_for_import(client, args.import_timeout, symbol_probe)
        report["import_ms"] = round((time.perf_counter() - import_started) * 1000, 3)
        if not report["import"]["ready"]:
            raise RuntimeError(f"project import did not become queryable: {report['import']}")

        summaries: list[dict[str, Any]] = []
        rpc_total = 0
        for spec in expectations:
            query_started = time.perf_counter()
            result, rpc_count = execute_query(client, project, spec)
            summary = summarize_query(project, spec, result)
            summary["elapsed_ms"] = round((time.perf_counter() - query_started) * 1000, 3)
            summary["rpc_count"] = rpc_count
            summaries.append(summary)
            rpc_total += rpc_count
        report["queries"] = summaries

        hot_timings: list[float] = []
        for index in range(20):
            spec = expectations[index % len(expectations)]
            query_started = time.perf_counter()
            execute_query(client, project, spec)
            hot_timings.append((time.perf_counter() - query_started) * 1000)
        report["hot_queries"] = {
            "count": len(hot_timings),
            "p50_ms": round(statistics.median(hot_timings), 3),
            "p95_ms": round(percentile(hot_timings, 0.95), 3),
            "max_ms": round(max(hot_timings), 3),
            "samples_ms": [round(value, 3) for value in hot_timings],
        }

        cancel_id = client.request_async("workspace/symbol", {"query": ""})
        client.notify("$/cancelRequest", {"id": cancel_id})
        cancel_outcome = "timeout"
        try:
            client.wait_response(cancel_id, 5)
            cancel_outcome = "response"
        except RuntimeError as error:
            cancel_outcome = f"error: {error}"
        except queue.Empty:
            cancel_outcome = "timeout"
        report["cancellation"] = {"sent": True, "outcome": cancel_outcome}
        report["rpc_count_initial_queries"] = rpc_total
    except Exception as error:
        report["fatal_error"] = str(error)
        report["traceback"] = traceback.format_exc()
    finally:
        report["lifecycle"] = client.close()
        report["memory"] = client.memory_report()
        report["total_ms"] = round((time.perf_counter() - started) * 1000, 3)
        report["notifications"] = client.notifications
        report["server_requests"] = client.server_requests
        report["reader_error"] = client.reader_error
        after_project = project_snapshot(project)
        report["project_changes"] = {
            "added": sorted(set(after_project) - set(before_project)),
            "removed": sorted(set(before_project) - set(after_project)),
            "modified": sorted(key for key in set(before_project) & set(after_project) if before_project[key] != after_project[key]),
        }
        report["workspace_after"] = directory_stats(workspace)
        report["passed"] = "fatal_error" not in report and all(item["passed"] for item in report.get("queries", []))
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"passed": report["passed"], "output": str(args.output), "fatal_error": report.get("fatal_error")}, ensure_ascii=False))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
