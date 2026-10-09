"""用真实 MCP 进程与 rg 对照 Java 样例；便携数据和仓库均在临时目录。"""

import argparse
import json
from pathlib import Path
import queue
import re
import shutil
import subprocess
import tempfile
import threading
import time


class Session:
    def __init__(self, exe, repo):
        self.process = subprocess.Popen(
            [str(exe), "mcp", str(repo)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, encoding="utf-8",
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        self.responses, self.errors, self.sequence = queue.Queue(), [], 0
        self.threads = [threading.Thread(target=self.read_output, daemon=True),
                        threading.Thread(target=self.read_errors, daemon=True)]
        for thread in self.threads:
            thread.start()

    def read_output(self):
        for line in self.process.stdout:
            self.responses.put(line)
        self.responses.put(None)

    def read_errors(self):
        self.errors.extend(self.process.stderr)

    def rpc(self, method, params=None):
        self.sequence += 1
        message = {"jsonrpc": "2.0", "id": self.sequence, "method": method}
        if params is not None:
            message["params"] = params
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()
        line = self.responses.get(timeout=20)
        assert line is not None, self.errors
        reply = json.loads(line)
        assert reply["id"] == self.sequence and "error" not in reply, reply
        return reply["result"]

    def tool(self, name, args=None):
        result = self.rpc("tools/call", {"name": name, "arguments": args or {}})
        assert not result.get("isError"), result
        return result.get("structuredContent") or json.loads(result["content"][0]["text"])

    def ready(self):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            status = self.tool("index_status")
            if status["status"] == "ready" and not status.get("indexing") and not status.get("needs_rebuild"):
                return status
            time.sleep(0.05)
        raise AssertionError(("index not ready", status, self.errors))

    def close(self):
        try:
            self.process.stdin.close()
            self.process.wait(timeout=10)
            assert self.process.returncode == 0, self.errors
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait()
            for thread in self.threads:
                thread.join(timeout=2)
            self.process.stdout.close()
            self.process.stderr.close()


SOURCES = {
    "PrivateOwner.java": """package demo;
public class PrivateOwner {
    private Object newResponseContext() { return null; }
}
""",
    "Tools.java": """package demo;
public class Tools {
    public static int parseInt(String value) { return Integer.parseInt(value); }
    public static String substring(String value) { return value.substring(0); }
    public static void mappingDataSet() {}
}
""",
    "Caller.java": """package demo;
import java.util.HashMap;
public class Caller extends MissingBase {
    public void run() {
        Tools.parseInt("1");
        Tools.parseInt("2");
        Integer.parseInt("3");
        this.newResponseContext();
        new HashMap();
        // Tools.parseInt("comment is not a call");
        String note = "Tools.parseInt(quoted text is not a call)";
    }
}
""",
    "StaticCaller.java": """package demo;
import static demo.Tools.mappingDataSet;
public class StaticCaller {
    public void runSecond() {
        mappingDataSet();
    }
}
""",
    "Service.java": """package demo;
public interface Service {
    void execute();
}
""",
    "State.java": """package demo;
public enum State { READY, DONE }
""",
    "SameFile.java": """package demo;
public class SameFile {
    private void local() {}
    public void invoke() {
        local();
    }
}
class Neighbor extends OtherBase {
    public void falseInvoke() {
        local();
    }
}
""",
}


def rg_matches(repo, pattern):
    result = subprocess.run(
        ["rg", "--json", "-g", "*.java", "-e", pattern, str(repo)],
        capture_output=True, text=True, encoding="utf-8",
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    assert result.returncode in (0, 1), result.stderr
    rows = []
    for line in result.stdout.splitlines():
        event = json.loads(line)
        if event["type"] == "match":
            data = event["data"]
            rows.append((Path(data["path"]["text"]).relative_to(repo).as_posix(),
                         data["line_number"], data["lines"]["text"]))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/khaslana.exe")
    exe = parser.parse_args().exe.resolve()
    assert exe.is_file(), exe
    assert shutil.which("rg") and shutil.which("git"), "需要 rg 和 git"
    with tempfile.TemporaryDirectory(prefix="khaslana-java-grep-") as temporary:
        root = Path(temporary).resolve()
        assert root.parent == Path(tempfile.gettempdir()).resolve()
        app, repo = root / "app", root / "repo"
        (app / "data").mkdir(parents=True)
        (app / "data/khaslana.sqlite3").touch()
        repo.mkdir()
        portable_exe = app / exe.name
        shutil.copy2(exe, portable_exe)
        subprocess.run(["git", "init", "--quiet", str(repo)], check=True,
                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        for name, source in SOURCES.items():
            path = repo / "src/main/java/demo" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source, encoding="utf-8")
        (repo / "pom.xml").write_text("<project><modelVersion>4.0.0</modelVersion><groupId>demo</groupId><artifactId>sample</artifactId><version>1</version></project>", encoding="utf-8")
        session = Session(portable_exe, repo)
        try:
            session.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "java-rg-comparison", "version": "1"}})
            status = session.ready()
            overview = session.tool("get_architecture")
            java_files = len(subprocess.check_output(["rg", "--files", "-g", "*.java", str(repo)], text=True, encoding="utf-8").splitlines())
            assert dict(overview["languages"]) == {"java": java_files, "xml": 1}, overview
            type_pattern = r"^\s*(?:public\s+)?(?:class|interface|enum)\s+(\w+)"
            method_pattern = r"^\s*(?:(?:public|private)\s+)?(?:static\s+)?(?:Object|String|int|void)\s+(\w+)\s*\("
            expected_types = {(path, line, re.search(type_pattern, text).group(1)) for path, line, text in rg_matches(repo, type_pattern)}
            expected_methods = {(path, line, re.search(method_pattern, text).group(1)) for path, line, text in rg_matches(repo, method_pattern)}
            indexed_types = session.tool("search_graph", {"name_pattern": "^(PrivateOwner|Tools|Caller|StaticCaller|Service|State|SameFile|Neighbor)$", "limit": 200})
            indexed_methods = session.tool("search_graph", {"label": "Method", "limit": 200})
            for indexed, expected in [(indexed_types, expected_types), (indexed_methods, expected_methods)]:
                assert not indexed["has_more"], indexed
                actual = {(hit["file_path"], hit["start_line"], hit["name"]) for hit in indexed["results"]}
                assert actual == expected, (actual, expected)
            static_locations = rg_matches(repo, r"^\s*Tools\.parseInt\s*\(")
            parsed = session.tool("trace_path", {"function_name": "parseInt", "direction": "inbound", "depth": 1})
            assert parsed["callers_total"] == 1, parsed
            sites = parsed["callers"][0]["call_sites"]
            assert sites["count"] == len(static_locations) == 2, sites
            assert {(sites["file_path"], line) for line in sites["lines"]} == {(path, line) for path, line, _ in static_locations}
            mapped = session.tool("trace_path", {"function_name": "mappingDataSet", "direction": "inbound", "depth": 1})
            assert mapped["callers_total"] == len(rg_matches(repo, r"^\s*mappingDataSet\s*\(")) == 1
            private = session.tool("trace_path", {"function_name": "newResponseContext", "direction": "inbound", "depth": 1})
            assert private["callers_total"] == 0
            local_calls = rg_matches(repo, r"^\s*local\s*\(")
            # 样例中第二个类继承外部基类；裸名称命中不能归给第一个类的私有方法。
            neighbor_line = next(line for _, line, text in rg_matches(repo, type_pattern) if "class Neighbor" in text)
            owned_local_calls = [(path, line) for path, line, _ in local_calls if line < neighbor_line]
            local = session.tool("trace_path", {"function_name": "local", "direction": "inbound", "depth": 1})
            assert local["callers_total"] == len(owned_local_calls) == 1
            assert local["callers"][0]["name"] == "invoke"
            assert local["callers"][0]["call_sites"]["lines"] == [line for _, line in owned_local_calls]
            output = {"java_files_rg_vs_index": [java_files, dict(overview["languages"])["java"]],
                      "type_definitions_rg_vs_index": [len(expected_types), indexed_types["total"]],
                      "method_definitions_rg_vs_index": [len(expected_methods), indexed_methods["total"]],
                      "static_call_sites_rg_vs_index": [len(static_locations), sites["count"]],
                      "static_call_site_lines": sites["lines"], "static_caller_nodes": parsed["callers_total"],
                      "static_import_calls_rg_vs_index": [1, mapped["callers_total"]],
                      "private_method_false_callers": private["callers_total"],
                      "local_raw_grep_hits": len(local_calls), "local_scope_checked_calls_vs_index": [len(owned_local_calls), local["callers_total"]],
                      "needs_rebuild": status["needs_rebuild"]}
            print(json.dumps(output, ensure_ascii=False, indent=2))
        finally:
            session.close()


if __name__ == "__main__":
    main()
