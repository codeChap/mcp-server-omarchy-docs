# mcp-server-omarchy-docs (Rust)

The whole project is Rust: two binaries built from this crate.

- **`mcp-server-omarchy-docs`** — offline Omarchy documentation MCP server:
  newline-delimited JSON-RPC over stdio, backed by a read-only SQLite FTS5
  knowledge base (`../data/kb.sqlite3`).
- **`build-kb`** — builds that knowledge base from raw sources fetched by
  `../scripts/fetch_sources.sh`.

Same 7 tools (`omarchy_search_docs`, `omarchy_get_page`, `omarchy_whats_new`,
`omarchy_find_config`, `omarchy_diff_releases`, `omarchy_check_updates`,
`omarchy_kb_info`), same tiering (manual > repo docs > configs > releases >
commits). SQLite (with FTS5) compiles into the binary via rusqlite's bundled
feature; there is no runtime dependency on Python, a system libsqlite3, or
OpenSSL.

## Build & test

```bash
cargo build --release        # → target/release/{mcp-server-omarchy-docs,build-kb}
cargo test                   # builder unit tests + end-to-end protocol tests
```

## Rebuild the knowledge base

```bash
../scripts/fetch_sources.sh  # once: clone basecamp/omarchy + fetch releases
./target/release/build-kb    # rebuild ../data/kb.sqlite3 (~6200 chunks, <1s)
```

Pass an optional root directory argument to build into a different checkout.

## Run

The server locates the knowledge base in this order:

1. `$OMARCHY_KB_ROOT` (must contain `data/kb.sqlite3`)
2. `--kb /path/to/project-root`
3. the binary's grandparent directory (so `server-rs/target/release/…`
   inside a checkout just works)
4. the crate checkout's parent directory

MCP client registration:

```json
{
  "mcpServers": {
    "omarchy-docs": {
      "command": "/absolute/path/to/mcp-server-omarchy-docs/server-rs/target/release/mcp-server-omarchy-docs"
    }
  }
}
```
