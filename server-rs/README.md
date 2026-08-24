# mcp-server-omarchy (Rust)

Native Rust port of [`../server/mcp_server.py`](../server/mcp_server.py) —
the same offline Omarchy documentation MCP server: newline-delimited JSON-RPC
over stdio, backed by the same read-only SQLite FTS5 knowledge base
(`../data/kb.sqlite3`).

Same 7 tools (`omarchy_search_docs`, `omarchy_get_page`, `omarchy_whats_new`,
`omarchy_find_config`, `omarchy_diff_releases`, `omarchy_check_updates`,
`omarchy_kb_info`), same tiering (manual > repo docs > configs > releases >
commits), same payloads. Tool responses are byte-for-byte identical to the
Python server — verified by `../scripts/diff_test_rs_vs_py.py`.

## Build

```bash
cargo build --release
# → target/release/mcp-server-omarchy (~5 MB)
```

Dependencies (bundled SQLite with FTS5, serde_json, ureq+rustls) compile into
the binary; there is no runtime dependency on a system libsqlite3, Python, or
OpenSSL.

## Run

The binary locates the knowledge base in this order:

1. `$OMARCHY_KB_ROOT` (must contain `data/kb.sqlite3`)
2. `--kb /path/to/project-root`
3. the binary's grandparent directory (so `server-rs/target/release/…`
   inside a checkout just works)
4. the crate checkout's parent directory

Then it serves JSON-RPC over stdio like any MCP server:

```json
{
  "mcpServers": {
    "omarchy-docs": {
      "command": "/absolute/path/to/mcp-server-omarchy/server-rs/target/release/mcp-server-omarchy"
    }
  }
}
```

## Test

```bash
cd ..
python3 scripts/test_mcp_rs.py        # protocol smoke test against this binary
python3 scripts/diff_test_rs_vs_py.py # differential test vs the Python server
```
