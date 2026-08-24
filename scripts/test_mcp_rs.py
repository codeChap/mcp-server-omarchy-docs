#!/usr/bin/env python3
"""End-to-end MCP protocol smoke test for the Rust server (server-rs).

Same assertions as scripts/test_mcp.py, pointed at the Rust binary.
"""
import json
import subprocess
import sys
from itertools import count
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SERVER = ROOT / "server-rs" / "target" / "release" / "mcp-server-omarchy"

passed = failed = 0


def send(proc, payload: dict) -> dict | None:
    is_notification = "id" not in payload
    proc.stdin.write(json.dumps(payload) + "\n")
    proc.stdin.flush()
    if is_notification:
        return None
    while True:
        line = proc.stdout.readline()
        if not line:
            raise RuntimeError("server closed stdout")
        resp = json.loads(line)
        if resp.get("id") == payload.get("id"):
            return resp


def check(label: str, cond: bool, detail: str = ""):
    global passed, failed
    if cond:
        passed += 1
        print(f"  ok  {label}")
    else:
        failed += 1
        print(f" FAIL {label} {detail}")


def call_tool(proc, name: str, args: dict | None = None) -> dict:
    resp = send(proc, {"jsonrpc": "2.0", "id": next(counter), "method": "tools/call",
                       "params": {"name": name, "arguments": args or {}}})
    assert "error" not in resp, f"RPC error: {resp['error']}"
    return json.loads(resp["result"]["content"][0]["text"])


counter = count(1)


def main():
    proc = subprocess.Popen(
        [str(SERVER)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True)

    resp = send(proc, {"jsonrpc": "2.0", "id": next(counter), "method": "initialize",
                       "params": {"protocolVersion": "2025-06-18",
                                  "capabilities": {},
                                  "clientInfo": {"name": "smoke", "version": "0"}}})
    info = resp["result"]["serverInfo"]
    check("initialize", info["name"] == "omarchy-docs", str(info))

    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})

    resp = send(proc, {"jsonrpc": "2.0", "id": next(counter), "method": "ping"})
    check("ping", resp.get("result") == {})

    resp = send(proc, {"jsonrpc": "2.0", "id": next(counter), "method": "tools/list"})
    tools = {t["name"] for t in resp["result"]["tools"]}
    expected = {"omarchy_search_docs", "omarchy_get_page", "omarchy_whats_new",
                "omarchy_find_config", "omarchy_diff_releases", "omarchy_check_updates",
                "omarchy_kb_info"}
    check("tools/list has all 7 tools", tools == expected, str(tools ^ expected))

    r = call_tool(proc, "omarchy_search_docs", {"query": "how do I switch themes?"})
    check("search finds theme docs",
          any(r_["source"] == "manual" and "theme" in r_["title"].lower()
              for r_ in r["results"]),
          json.dumps([r_["title"] for r_ in r["results"][:3]]))

    r = call_tool(proc, "omarchy_search_docs",
                  {"query": "themes", "source_filter": "manual"})
    check("manual-filtered search ranks Themes chapter",
          any(r_["title"] == "Themes" and r_["section"] == "Themes"
              for r_ in r["results"][:4]),
          json.dumps([r_["title"] for r_ in r["results"][:4]]))

    r = call_tool(proc, "omarchy_search_docs", {"query": "screenshots"})
    check("search finds screenshot chapter",
          any("Screenshot" in r_["title"] for r_ in r["results"]),
          json.dumps([r_["title"] for r_ in r["results"][:3]]))

    r = call_tool(proc, "omarchy_get_page", {"name_or_url": "Hotkeys"})
    check("get_page by title", r.get("chars", 0) > 3000,
          str({k: r.get(k) for k in ("error", "candidates")}))

    r = call_tool(proc, "omarchy_get_page",
                  {"name_or_url": "https://omarchy.org/manual/dual-boot-install/"})
    check("get_page by URL", "Dual Boot" in r.get("title", ""), str(r.get("error")))

    r = call_tool(proc, "omarchy_whats_new", {})
    check("whats_new latest is 4.0.0", r.get("version") == "4.0.0", str(r.get("version")))
    check("whats_new mentions Quattro features", len(r.get("notes", "")) > 10_000)

    r = call_tool(proc, "omarchy_whats_new", {"since": "2026-08-01"})
    check("whats_new since covers v4 only-ish",
          r.get("count", 0) >= 1 and all(
              rel["published"] > "2026-08-01" for rel in r["releases"]))

    r = call_tool(proc, "omarchy_find_config", {"app": "hyprland"})
    paths = [c["path"] for c in r["config_files_by_path"]]
    check("find_config hyprland hits hyprland.lua",
          any(p.endswith("hypr/hyprland.lua") for p in paths), str(paths[:4]))

    r = call_tool(proc, "omarchy_kb_info")
    check("kb_info counts", r.get("chunks", {}).get("manual", 0) > 150, str(r.get("chunks")))

    proc.terminate()
    print(f"\n{passed} passed, {failed} failed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
