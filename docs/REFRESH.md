# Refreshing the knowledge base

The server is a **point-in-time snapshot**. At query time it never touches the
network; the one exception is `omarchy_check_updates`, which makes a single
GitHub API call when you invoke it and returns `UP TO DATE` or `OUTDATED`
plus the refresh command.

## Three ways to stay current

1. **Check**: call `omarchy_check_updates` (or read `built_at` /
   `latest_release` from `omarchy_kb_info`).
2. **Rebuild locally** (always current, needs git + network once):
   ```bash
   scripts/fetch_sources.sh && python3 scripts/build_kb.py
   ```
3. **Download a prebuilt snapshot**: this repo runs a weekly GitHub Action
   (Mondays) that rebuilds, smoke-tests, and attaches `kb.sqlite3` to the
   [`kb` release](https://github.com/codeChap/mcp-server-omarchy/releases/tag/kb).
   Drop it into `data/` next to the server — no fetch/build needed.


## What "current" means

- The KB pins to the newest release tag at fetch time (currently v4.0.0).
- Manual content comes from the repo's `manual/*.md` — between releases this
  can drift ahead of omarchy.org, which is fine (it's the same source).
- Commit tier tracks master, so post-release work is searchable immediately
  after a refresh.

## When to refresh

- A new Omarchy release is announced (`omarchy_whats_new` shows an old tag).
- The user's installed version is newer than `latest_release`.
- Before debugging anything version-sensitive on a fresh machine.

## Schema changes

`build_kb.py` recreates `data/kb.sqlite3` from scratch each run; there is no
migration path. Bump expectations in `scripts/test_mcp.py` when chunk counts
shift massively.
