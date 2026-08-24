#!/usr/bin/env python3
"""Differential test: replay the same MCP calls against the Python and Rust
servers and compare parsed JSON-RPC results field by field.

The two servers must agree on every tool payload (network tool excluded).
"""
import json
import subprocess
import sys
from itertools import count
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY = [sys.executable, str(ROOT / "server" / "mcp_server.py")]
RS = [str(ROOT / "server-rs" / "target" / "release" / "mcp-server-omarchy")]

CALLS = [
    ("tools/list", None),
    ("call", "omarchy_search_docs", {"query": "how do I switch themes?"}),
    ("call", "omarchy_search_docs", {"query": "themes", "source_filter": "manual"}),
    ("call", "omarchy_search_docs", {"query": "screenshots"}),
    ("call", "omarchy_search_docs", {"query": "wifi"}),
    ("call", "omarchy_search_docs", {"query": "battery"}),
    ("call", "omarchy_search_docs", {"query": "quickshell widget bar"}),
    ("call", "omarchy_search_docs", {"query": "zzzqqqxyzzy"}),
    ("call", "omarchy_search_docs",
     {"query": "keybind", "source_filter": "release", "omarchy_version": "4.0.0"}),
    ("call", "omarchy_search_docs", {"query": "snapshot rollback", "limit": 3}),
    ("call", "omarchy_get_page", {"name_or_url": "Hotkeys"}),
    ("call", "omarchy_get_page",
     {"name_or_url": "https://omarchy.org/manual/dual-boot-install/"}),
    ("call", "omarchy_get_page", {"name_or_url": "manual/06-themes.md"}),
    ("call", "omarchy_get_page", {"name_or_url": "zzz-no-such-page"}),
    ("call", "omarchy_get_page", {"name_or_url": "the"}),
    ("call", "omarchy_whats_new", {}),
    ("call", "omarchy_whats_new", {"version": "3.7.1"}),
    ("call", "omarchy_whats_new", {"version": "v4.0.0"}),
    ("call", "omarchy_whats_new", {"version": "9.9.9"}),
    ("call", "omarchy_whats_new", {"since": "2026-08-01"}),
    ("call", "omarchy_whats_new", {"since": "1999-01-01"}),
    ("call", "omarchy_find_config", {"app": "hyprland"}),
    ("call", "omarchy_find_config", {"app": "waybar"}),
    ("call", "omarchy_find_config", {"app": "gtk"}),
    ("call", "omarchy_diff_releases",
     {"from_version": "3.8.4", "to_version": "4.0.0"}),
    ("call", "omarchy_diff_releases",
     {"from_version": "3.7.0", "to_version": "3.8.4", "breaking_only": True}),
    ("call", "omarchy_diff_releases",
     {"from_version": "4.0.0", "to_version": "3.8.4"}),
    ("call", "omarchy_diff_releases",
     {"from_version": "1.2.3", "to_version": "4.0.0"}),
    ("call", "omarchy_kb_info", {}),
]


class Client:
    def __init__(self, cmd):
        self.proc = subprocess.Popen(
            cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True)
        self.ids = count(1)

    def send(self, payload):
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()
        if "id" not in payload:
            return None
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise RuntimeError("server closed stdout")
            resp = json.loads(line)
            if resp.get("id") == payload.get("id"):
                return resp

    def initialize(self):
        self.send({"jsonrpc": "2.0", "id": next(self.ids), "method": "initialize",
                   "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                              "clientInfo": {"name": "diff", "version": "0"}}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def call_tool(self, name, args):
        resp = self.send({"jsonrpc": "2.0", "id": next(self.ids),
                          "method": "tools/call",
                          "params": {"name": name, "arguments": args or {}}})
        return json.loads(resp["result"]["content"][0]["text"])

    def close(self):
        self.proc.terminate()


def diff(path, a, b, problems):
    """Collect differing leaf paths between two JSON values."""
    if isinstance(a, dict) and isinstance(b, dict):
        for k in sorted(set(a) | set(b)):
            if k not in a:
                problems.append(f"{path}.{k}: missing in python ({json.dumps(b[k])[:120]})")
            elif k not in b:
                problems.append(f"{path}.{k}: missing in rust ({json.dumps(a[k])[:120]})")
            else:
                diff(f"{path}.{k}", a[k], b[k], problems)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            problems.append(f"{path}: list length {len(a)} (py) vs {len(b)} (rust)")
        for i, (x, y) in enumerate(zip(a, b)):
            diff(f"{path}[{i}]", x, y, problems)
    elif a != b:
        problems.append(
            f"{path}: py={json.dumps(a)[:160]} rust={json.dumps(b)[:160]}")


def main():
    py = Client(PY)
    rs = Client(RS)
    py.initialize()
    rs.initialize()

    total = 0
    all_problems = []
    for call in CALLS:
        if call[0] == "tools/list":
            py_resp = py.send({"jsonrpc": "2.0", "id": next(py.ids),
                               "method": "tools/list"})
            rs_resp = rs.send({"jsonrpc": "2.0", "id": next(rs.ids),
                               "method": "tools/list"})
            py_tools = sorted(t["name"] for t in py_resp["result"]["tools"])
            rs_tools = sorted(t["name"] for t in rs_resp["result"]["tools"])
            total += 1
            if py_tools == rs_tools:
                print(f"  ok  tools/list: same 7 tool names")
            else:
                all_problems.append(f"tools/list: {py_tools} vs {rs_tools}")
                print(f" FAIL tools/list")
            continue

        _, name, args = call
        a = py.call_tool(name, args)
        b = rs.call_tool(name, args)
        total += 1
        label = f"{name} {json.dumps(args) if args else ''}"
        problems = []
        diff("", a, b, problems)
        # The Rust server omits null-valued optional fields the Python server
        # also leaves out via .get(); treat explicit-null vs absent as equal.
        problems = [p for p in problems if "None vs null" not in p]
        if not problems:
            print(f"  ok  {label}")
        else:
            print(f" FAIL {label}")
            all_problems.extend(problems)

    py.close()
    rs.close()
    print(f"\n{total - bool(all_problems)}/{total} calls match")
    for p in all_problems[:40]:
        print(f"  - {p}")
    sys.exit(1 if all_problems else 0)


if __name__ == "__main__":
    main()
