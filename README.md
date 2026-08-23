# mcp-server-omarchy

Offline documentation MCP server for **Omarchy v4.0.0 "Quattro"**.
Zero dependencies — Python 3.10+ stdlib, SQLite FTS5, JSON-RPC over stdio.
No Docker, no ChromaDB, no embeddings, no network at query time.

Native alternative to [Zeus-Deus/omarchy-mcp](https://github.com/Zeus-Deus/omarchy-mcp)
(Docker + ChromaDB + sentence-transformers): same corpus idea, none of the
container machinery. Thanks to that project for the original blueprint.

All indexed content comes from [basecamp/omarchy](https://github.com/basecamp/omarchy)
(MIT) and omarchy.org — this server only indexes and serves it. Code is MIT
licensed: see [LICENSE](LICENSE).

## Knowledge base

| Source | Chunks | What |
|---|---:|---|
| `manual` | ~186 | Official Quattro manual, linked to live omarchy.org URLs |
| `repo-docs` | ~66 | Maintainer internals: theming engine, CLI router, update process |
| `config` | ~109 | Every shipped default config (hypr/*.lua, foot.ini, /etc overrides…) |
| `release` | ~201 | All GitHub release notes through v4.0.0 |
| `commit` | ~5600 | Full git history — design rationale lives in commit messages |

Results are tiered so manual answers outrank configs, which outrank historical
releases and commits, regardless of raw text score. Trust model:
[docs/TIERS.md](docs/TIERS.md).

## Setup

```bash
scripts/fetch_sources.sh      # clone basecamp/omarchy + fetch release notes
python3 scripts/build_kb.py   # build data/kb.sqlite3 (~6200 chunks)
python3 scripts/test_mcp.py   # end-to-end protocol smoke test
```

## MCP client registration

```json
{
  "mcpServers": {
    "omarchy-docs": {
      "command": "python3",
      "args": ["/absolute/path/to/mcp-server-omarchy/server/mcp_server.py"]
    }
  }
}
```

## Tools

| Tool | Use when |
|---|---|
| `omarchy_search_docs` | Any question. Start here. Zero/thin results include `did_you_mean` + an escalation ladder for finding the answer elsewhere. |
| `omarchy_get_page` | You know the page: title (`"Hotkeys"`), URL, or repo path. |
| `omarchy_whats_new` | Latest release (no args), a specific version, or everything since a date. |
| `omarchy_diff_releases` | "I broke my system with an update" — what changed between two versions; `breaking_only=true`. |
| `omarchy_find_config` | Where a setting/default/keybind lives. |
| `omarchy_kb_info` | Check snapshot freshness **before** trusting version-sensitive answers. |

Querying tips (lexical search, exact vocabulary matters):
[docs/QUERYING.md](docs/QUERYING.md).
When there are no docs for something: [docs/AGENT_PLAYBOOK.md](docs/AGENT_PLAYBOOK.md).
Where humans get help: [docs/GETTING_HELP.md](docs/GETTING_HELP.md).

## Updating after a new Omarchy release

```bash
scripts/fetch_sources.sh && python3 scripts/build_kb.py
```

See [docs/REFRESH.md](docs/REFRESH.md) for snapshot semantics.
