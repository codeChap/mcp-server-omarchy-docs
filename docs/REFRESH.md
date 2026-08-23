# Refreshing the knowledge base

The server is a **point-in-time snapshot**. It never touches the network at
query time — that's a feature. Freshness is visible via `omarchy_kb_info`
(`latest_release`, `built_from_commit`).

## Rebuild

```bash
scripts/fetch_sources.sh      # git pull basecamp/omarchy + releases API
python3 scripts/build_kb.py   # rebuild data/kb.sqlite3 (~6200 chunks)
```

Takes under a minute; the server can stay running (it opens the DB read-only
per connection; restart it after a rebuild to be safe).

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
