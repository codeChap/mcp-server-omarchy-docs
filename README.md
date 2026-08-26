# mcp-server-omarchy-docs

Offline documentation MCP server for **Omarchy v4.0.0 "Quattro"**.
100% Rust: one static binary (bundled SQLite with FTS5, JSON-RPC over stdio).
No Docker, no ChromaDB, no embeddings, no network at query time — no Python.

Native alternative to [Zeus-Deus/omarchy-mcp](https://github.com/Zeus-Deus/omarchy-mcp)
(Docker + ChromaDB + sentence-transformers): same corpus idea, none of the
container machinery. Thanks to that project for the original blueprint.

Indexed content comes from [basecamp/omarchy](https://github.com/basecamp/omarchy)
(MIT), omarchy.org, and the [Omarchy Plugins](https://omarchyplugins.com/develop.html)
authoring guides — this server only indexes and serves it. Code is MIT
licensed: see [LICENSE](LICENSE).

## Knowledge base

| Source | Chunks | What |
|---|---:|---|
| `manual` | ~186 | Official Quattro manual, linked to live omarchy.org URLs |
| `repo-docs` | ~66 | Maintainer internals: theming engine, CLI router, update process |
| `plugin-dev` | ~10 | [omarchyplugins.com](https://omarchyplugins.com/develop.html) authoring guides (clone, manifest, QML, publish) |
| `config` | ~109 | Every shipped default config (hypr/*.lua, foot.ini, /etc overrides…) |
| `release` | ~201 | All GitHub release notes through v4.0.0 |
| `commit` | ~5600 | Full git history — design rationale lives in commit messages |

Results are tiered so manual answers outrank configs, which outrank historical
releases and commits, regardless of raw text score. Trust model:
[docs/TIERS.md](docs/TIERS.md).

## Setup

```bash
scripts/fetch_sources.sh                    # clone basecamp/omarchy + fetch release notes
cd server-rs && cargo build --release       # build both binaries (~5 MB, static SQLite)
./target/release/build-kb                   # build ../data/kb.sqlite3 (~6200 chunks)
cargo test                                  # unit + end-to-end protocol tests
```

## MCP client registration

```json
{
  "mcpServers": {
    "omarchy-docs": {
      "command": "/absolute/path/to/mcp-server-omarchy-docs/server-rs/target/release/mcp-server-omarchy-docs"
    }
  }
}
```

The binary resolves `data/kb.sqlite3` relative to its parent directory, or set
`OMARCHY_KB_ROOT=/path/to/mcp-server-omarchy-docs` (or pass `--kb <root>`).

## Tools

| Tool | Use when |
|---|---|
| `omarchy_search_docs` | Any question. Start here. Zero/thin results include `did_you_mean` + an escalation ladder for finding the answer elsewhere. |
| `omarchy_get_page` | You know the page: title (`"Hotkeys"`), URL, or repo path. |
| `omarchy_whats_new` | Latest release (no args), a specific version, or everything since a date. |
| `omarchy_diff_releases` | "I broke my system with an update" — what changed between two versions; `breaking_only=true`. |
| `omarchy_find_config` | Where a setting/default/keybind lives. |
| `omarchy_check_updates` | Is the KB behind upstream? One GitHub API call, only when invoked. |
| `omarchy_kb_info` | Snapshot age/commit/chunks — freshness at a glance, offline. |

Querying tips (lexical search, exact vocabulary matters):
[docs/QUERYING.md](docs/QUERYING.md).
When there are no docs for something: [docs/AGENT_PLAYBOOK.md](docs/AGENT_PLAYBOOK.md).
Where humans get help: [docs/GETTING_HELP.md](docs/GETTING_HELP.md).

## Updating after a new Omarchy release

```bash
scripts/fetch_sources.sh && (cd server-rs && cargo build --release && ./target/release/build-kb)
```

Or grab the prebuilt weekly snapshot from the
[`kb` release](https://github.com/codeChap/mcp-server-omarchy-docs/releases/tag/kb).
See [docs/REFRESH.md](docs/REFRESH.md) for snapshot semantics.
