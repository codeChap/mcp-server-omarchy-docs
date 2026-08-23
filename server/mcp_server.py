#!/usr/bin/env python3
"""mcp-server-omarchy — offline Omarchy (Quattro/v4) documentation MCP server.

Zero third-party dependencies: Python 3.10+ stdlib only, SQLite FTS5 for
search, newline-delimited JSON-RPC over stdio per the MCP spec.

Run: python3 server/mcp_server.py
"""
from __future__ import annotations

import json
import re
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DB_PATH = ROOT / "data" / "kb.sqlite3"
SERVER_NAME = "omarchy-docs"
SERVER_VERSION = "1.0.0"

STOPWORDS = {
    "how", "do", "does", "i", "a", "an", "the", "to", "in", "on", "for", "of",
    "my", "me", "is", "are", "it", "its", "what", "whats", "when", "where",
    "which", "can", "with", "and", "or", "get", "got",
}

# Result tiers: the current manual beats repo internals beats shipped configs
# beats historical release notes, regardless of raw bm25 score.
TIERS = (
    ("core", ("manual", "repo-docs")),
    ("configs", ("config",)),
    ("releases", ("release",)),
    ("history", ("commit",)),
)

TIER_LIMITS = {"core": 6, "configs": 3, "releases": 4, "history": 3}

# Curated Omarchy-concept -> upstream-docs URL mapping (see data/upstream_map.json).
UPSTREAM_MAP_PATH = ROOT / "data" / "upstream_map.json"
SYNONYMS_PATH = ROOT / "data" / "synonyms.json"


def log(msg: str) -> None:
    print(f"[{SERVER_NAME}] {msg}", file=sys.stderr)


def tokenize(query: str) -> list[str]:
    raw = re.split(r"[^\w+#.-]+", query.lower())
    return [t for t in raw if t and t not in STOPWORDS]


def fts_escape(token: str) -> str:
    """Quote a token for FTS5 so operators/punctuation stay literal."""
    return '"' + token.replace('"', '""') + '"'


def load_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def expand_synonyms(tokens: list[str], synonyms: dict) -> list[str]:
    extra = []
    for t in tokens:
        for alt in synonyms.get(t, []):
            for w in alt.split():
                if w not in tokens and w not in extra:
                    extra.append(w)
    return extra


def suggest_terms(con: sqlite3.Connection, tokens: list[str]) -> list[str]:
    """Nearest vocabulary terms for zero-result queries ('did you mean')."""
    out = []
    for t in tokens:
        if len(t) < 4 or t.endswith("*"):
            continue
        prefix = t[: max(4, len(t) - 2)]
        rows = con.execute(
            "SELECT term FROM terms WHERE term LIKE ? LIMIT 3",
            (prefix + "%",)).fetchall()
        out.extend({r["term"] for r in rows} - {t})
    return out[:5]


ESCALATION = {
    "retry_hints": [
        "Use Omarchy's exact vocabulary: quickshell, keybind, theme, snapshot, factory reset",
        "Try one distinctive noun instead of a full sentence",
        "Config/keybind/default question? Use omarchy_find_config",
        "'When did X change?' questions belong in omarchy_whats_new",
    ],
    "next_steps": (
        "Thin results may mean the topic is undocumented. Escalate: "
        "(1) read the source at github.com/basecamp/omarchy — bin/ scripts and "
        "git log are the real docs for internals; "
        "(2) upstream docs — quickshell.org/docs (v4 shell), wiki.hypr.land "
        "(window manager options), wiki.archlinux.org (everything below); "
        "(3) search closed issues at github.com/basecamp/omarchy/issues; "
        "(4) ask on the Omarchy Discord discord.gg/tXFUdasqhY with omarchy-debug output attached. "
        "Never invent Omarchy-specific behavior from general Arch/Hyprland knowledge; "
        "if unverified, say so."),
}


class KnowledgeBase:
    def __init__(self, db_path: Path):
        if not db_path.exists():
            raise SystemExit(
                f"Knowledge base missing at {db_path}. Run scripts/build_kb.py first.")
        self.con = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        self.con.row_factory = sqlite3.Row
        self.synonyms = load_json(SYNONYMS_PATH)
        self.upstream_map = load_json(UPSTREAM_MAP_PATH)

    def meta(self) -> dict:
        return dict(self.con.execute("SELECT key, value FROM meta"))

    def counts(self) -> dict:
        return dict(self.con.execute(
            "SELECT source, COUNT(*) FROM chunks GROUP BY source").fetchall())

    # ------------------------------------------------------------------ search

    def _run_match(self, match_expr: str, sources: tuple[str, ...], limit: int):
        placeholders = ",".join("?" for _ in sources)
        return self.con.execute(
            f"""
            SELECT c.source, c.title, c.section, c.body, c.url, c.path,
                   c.version, c.published,
                   bm25(chunks_fts, 6.0, 3.0, 1.0) AS rank,
                   highlight(chunks_fts, 2, '<<', '>>') AS snippet
            FROM chunks_fts f JOIN chunks c ON c.id = f.rowid
            WHERE chunks_fts MATCH ? AND c.source IN ({placeholders})
            ORDER BY rank LIMIT ?""",
            (match_expr, *sources, limit),
        ).fetchall()

    def search(self, query: str, source_filter: str | None = None,
               omarchy_version: str | None = None, limit: int = 8):
        tokens = tokenize(query)
        if not tokens:
            return []

        tiers = TIERS
        if source_filter == "omarchy":
            tiers = (("core", ("manual", "repo-docs")), ("releases", ("release",)))
        elif source_filter in ("manual", "docs"):
            tiers = (("core", ("manual", "repo-docs")),)
        elif source_filter == "config":
            tiers = (("configs", ("config",)),)
        elif source_filter == "release":
            tiers = (("releases", ("release",)),)

        def collect(expr: str, exactness: str) -> list[dict]:
            """Best-of-each-tier in priority order (manual > configs > releases > commits)."""
            results, seen = [], set()
            for tier_name, sources in tiers:
                cap = TIER_LIMITS.get(tier_name, 4)
                taken = 0
                for row in self._run_match(expr, sources, cap * 2 + limit):
                    key = (row["source"], row["title"], row["section"])
                    if key in seen:
                        continue
                    seen.add(key)
                    rec = dict(row)
                    rec["tier"] = tier_name
                    rec["match"] = exactness
                    results.append(rec)
                    taken += 1
                    if taken >= cap:
                        break
            return results

        synonyms = expand_synonyms(tokens, self.synonyms)
        and_expr = " AND ".join(fts_escape(t) + "*" for t in tokens)
        or_terms = tokens + [t for t in synonyms if not any(t.startswith(b) or b.startswith(t) for b in tokens)]
        or_expr = " OR ".join(fts_escape(t) + "*" for t in or_terms)

        out = collect(and_expr, "exact")
        if len(out) < limit:
            seen_keys = {(r["source"], r["title"], r["section"]) for r in out}
            for rec in collect(or_expr, "partial"):
                key = (rec["source"], rec["title"], rec["section"])
                if key not in seen_keys:
                    seen_keys.add(key)
                    out.append(rec)

        # Tag results with curated upstream links when the query matches.
        see_also = []
        q_lower = query.lower()
        for phrase, url in self.upstream_map.items():
            if all(w in q_lower for w in phrase.split()):
                see_also.append(url)
        if see_also:
            for rec in out:
                rec["see_also"] = see_also[:3]

        out = out[:limit]

        if omarchy_version:
            wanted = omarchy_version.lstrip("v")
            out = [r for r in out
                   if r["source"] != "release" or (r["version"] or "").lstrip("v") == wanted]
        return out

    # ------------------------------------------------------------------ pages

    def get_page(self, name_or_url: str):
        q = name_or_url.strip()
        norm_q = re.sub(r"[^a-z0-9]+", "", q.lower()).strip()

        # By live URL (/manual/...) or repo-relative path (manual/06-themes.md)
        row = None
        if q.startswith(("http://", "https://")):
            row = self.con.execute(
                "SELECT DISTINCT title FROM chunks WHERE url = ?", (q,)).fetchone()
            if row:
                return self._page_by_title(row["title"])
        if "/" in q or q.endswith(".md"):
            rows = self.con.execute(
                "SELECT DISTINCT title FROM chunks WHERE path LIKE ? ORDER BY title",
                (f"%{q}%",)).fetchall()
            if len(rows) == 1:
                return self._page_by_title(rows[0]["title"])
            if rows:
                return {"error": "ambiguous", "candidates": [r["title"] for r in rows]}
            return {"error": f"No page matching {q!r}"}

        # By (partial) title across manual + repo docs
        rows = self.con.execute(
            """
            SELECT DISTINCT title FROM chunks WHERE source IN ('manual','repo-docs')
            """).fetchall()
        matches = []
        for r in rows:
            norm_t = re.sub(r"[^a-z0-9]+", "", r["title"].lower())
            if norm_q and (norm_q == norm_t or norm_q in norm_t or norm_t.startswith(norm_q)):
                matches.append(r["title"])
        if len(matches) == 1:
            return self._page_by_title(matches[0])
        if not matches:
            return {"error": f"No page titled {name_or_url!r}",
                    "hint": "Use omarchy_search_docs first, then pass result.title or .url"}
        return {"error": "ambiguous", "candidates": sorted(matches)}

    def _page_by_title(self, title: str):
        rows = self.con.execute(
            """
            SELECT source, title, section, body, url, path FROM chunks
            WHERE title = ? AND source IN ('manual','repo-docs')
            ORDER BY id""",
            (title,),
        ).fetchall()
        if not rows:
            return {"error": f"No page titled {title!r}"}
        parts, url, path, source = [], "", "", ""
        for r in rows:
            source, url, path = r["source"], r["url"], r["path"]
            if r["section"] and r["body"].startswith("#"):
                parts.append(r["body"])
            elif r["section"]:
                parts.append(f"## {r['section']}\n\n{r['body']}")
            else:
                parts.append(r["body"])
        text = "\n\n".join(parts)
        return {"source": source, "title": title, "path": path, "url": url,
                "chars": len(text), "markdown": text}

    # ---------------------------------------------------------------- releases

    RELEASE_CAP = 40_000

    def whats_new(self, version: str | None = None, since: str | None = None):
        if version:
            v = version.lstrip("v")
            rows = self.con.execute(
                """
                SELECT version, published, section, body, url FROM chunks
                WHERE source='release' AND REPLACE(version,'.','') LIKE ?
                  OR source='release' AND version = ?
                ORDER BY published DESC, id
                """,
                (f"%{v.replace('.', '')}%", f"v{version.lstrip('v')}"),
            ).fetchall()
            rows = [r for r in rows if r["version"].lstrip("v") == v]
            if not rows:
                avail = [r[0] for r in self.con.execute(
                    "SELECT DISTINCT version FROM chunks WHERE source='release' "
                    "ORDER BY published DESC")]
                return {"error": f"No release {version!r}", "available": avail[:15]}
            return self._format_release(rows[0]["version"], rows)

        if since:
            rows = self.con.execute(
                """
                SELECT version, published, section, body, url FROM chunks
                WHERE source='release' AND published > ?
                GROUP BY version ORDER BY published DESC, id
                """,
                (since,),
            ).fetchall()
            versions = [r["version"] for r in rows]
            if not versions:
                return {"error": f"No releases after {since}"}
            all_rows = self.con.execute(
                "SELECT version, published, section, body, url FROM chunks "
                "WHERE source='release' ORDER BY published DESC, id").fetchall()
            out = []
            for v in versions:
                vr = [r for r in all_rows if r["version"] == v]
                out.append(self._format_release(v, vr))
            return {"count": len(out), "releases": out}

        latest = self.con.execute(
            "SELECT version, MAX(published) FROM chunks WHERE source='release'"
        ).fetchone()
        if not latest or not latest[0]:
            return {"error": "No release notes indexed"}
        rows = self.con.execute(
            "SELECT version, published, section, body, url FROM chunks "
            "WHERE source='release' AND version=? ORDER BY id",
            (latest[0],)).fetchall()
        return self._format_release(latest[0], rows)

    def _format_release(self, version: str, rows):
        url = next((r["url"] for r in rows if r["url"]), "")
        published = next((r["published"] for r in rows if r["published"]), "")
        body = "\n\n".join(
            (f"## {r['section']}\n\n{r['body']}" if r["section"] else r["body"])
            for r in rows)
        truncated = False
        if len(body) > self.RELEASE_CAP:
            body, truncated = body[:self.RELEASE_CAP], True
        return {"version": version, "published": published, "url": url,
                "truncated": truncated, "notes": body}

    def diff_releases(self, from_version: str, to_version: str,
                      breaking_only: bool = False):
        """Summarize what changed between two releases (inclusive of `to`)."""
        versions = [r[0] for r in self.con.execute(
            "SELECT DISTINCT version FROM chunks WHERE source='release' "
            "ORDER BY published")]
        strip = lambda v: v.lstrip("v")  # noqa: E731
        f, t = strip(from_version), strip(to_version)
        if f not in versions and f"v{f}" not in versions:
            return {"error": f"Unknown version {from_version!r}",
                    "known": versions[-10:]}
        if t not in versions and f"v{t}" not in versions:
            return {"error": f"Unknown version {to_version!r}",
                    "known": versions[-10:]}

        def key(v):
            return [int(x) for x in v.split(".") if x.isdigit()]

        lo, hi = key(f), key(t)
        between = [v for v in versions
                   if v.lstrip("v") != f and lo < key(v.lstrip("v")) <= hi]
        if not between:
            return {"error": f"No releases between {f} and {t}",
                    "hint": "They may be the same or in the wrong order"}

        all_rows = self.con.execute(
            "SELECT version, published, section, body, url FROM chunks "
            "WHERE source='release' ORDER BY published DESC, id").fetchall()
        releases = []
        for v in between:
            vr = [r for r in all_rows if r["version"] == v]
            rel = self._format_release(v, vr)
            if breaking_only:
                text = rel["notes"].lower()
                if not any(m in text for m in
                           ("breaking", "removed", "renamed", "migration",
                            "moved to", "now requires", "incompatible")):
                    continue
            releases.append(rel)
        if breaking_only and not releases:
            return {"from": f, "to": t, "releases": [],
                    "note": "No release in this range flagged breaking changes "
                            "by keyword; call without breaking_only to see all."}
        return {"from": f, "to": t, "count": len(releases), "releases": releases}

    # ----------------------------------------------------------------- updates

    RELEASES_API = "https://api.github.com/repos/basecamp/omarchy/releases?per_page=5"

    def check_updates(self) -> dict:
        """Compare the KB snapshot against upstream. Network call on invoke."""
        import urllib.request

        m = self.meta()
        local_latest = (m.get("latest_release") or "").lstrip("v")
        try:
            req = urllib.request.Request(
                self.RELEASES_API,
                headers={"Accept": "application/vnd.github+json",
                         "User-Agent": "mcp-server-omarchy"})
            with urllib.request.urlopen(req, timeout=10) as resp:
                releases = json.loads(resp.read().decode("utf-8"))
        except Exception as e:  # noqa: BLE001 — network failures are expected
            return {"error": f"Could not reach GitHub API: {e}",
                    "local": self._snapshot_info(),
                    "manual_check": "https://github.com/basecamp/omarchy/releases"}

        upstream = [r["tag_name"].lstrip("v") for r in releases
                    if r.get("tag_name")]
        newest = upstream[0] if upstream else None
        info = self._snapshot_info()
        if newest and newest != local_latest:
            key = lambda v: [int(x) for x in v.split(".") if x.isdigit()]  # noqa: E731
            outdated = bool(newest and local_latest and key(newest) > key(local_latest))
            info.update({
                "upstream_newest_release": newest,
                "status": ("OUTDATED — refresh now" if outdated
                           else "KB is ahead of tagged releases (unreleased commits indexed)"),
                "refresh": "scripts/fetch_sources.sh && python3 scripts/build_kb.py",
                "recent_upstream_releases": upstream[:5],
            })
        else:
            info.update({
                "upstream_newest_release": newest,
                "status": "UP TO DATE",
            })
        return info

    def _snapshot_info(self) -> dict:
        m = self.meta()
        return {
            "local_snapshot": {
                "latest_release": m.get("latest_release"),
                "built_from_commit": m.get("built_from_commit"),
                "built_at": m.get("built_at", "unknown (rebuild to stamp)"),
                "chunks": sum(self.counts().values()),
            },
        }


    def find_config(self, app: str):
        like = f"%{app}%"
        by_path = self.con.execute(
            """
            SELECT path, body FROM chunks WHERE source='config'
              AND (path LIKE ? OR title LIKE ?)
            ORDER BY LENGTH(path) LIMIT 12""",
            (like, like),
        ).fetchall()

        content_hits = self.search(app, source_filter="config", limit=6)

        manual_hits = self.con.execute(
            """
            SELECT c.title AS page_title, c.section, c.url,
                   highlight(chunks_fts, 2, '<<', '>>') AS snippet
            FROM chunks_fts f JOIN chunks c ON c.id=f.rowid
            WHERE chunks_fts MATCH ? AND c.source IN ('manual','repo-docs')
              AND (c.body LIKE '%~/.config%' OR c.body LIKE '%/etc/%')
            ORDER BY rank LIMIT 6""",
            (fts_escape(app) + "*",),
        ).fetchall()

        return {
            "app": app,
            "config_files_by_path": [
                {"path": r["path"], "excerpt": r["body"][:600]} for r in by_path],
            "config_files_by_content": [
                {"path": r["path"],
                 "snippet": r["snippet"][:600]} for r in content_hits],
            "doc_references": [
                {"page": r["page_title"], "section": r["section"],
                 "url": r["url"], "snippet": r["snippet"]} for r in manual_hits],
        }


# ------------------------------------------------------------------ MCP layer

TOOLS = [
    {
        "name": "omarchy_search_docs",
        "description": (
            "Search the offline Omarchy v4 (Quattro) knowledge base: the official "
            "manual, repo developer docs, shipped default configs, GitHub release "
            "notes, and full git commit history. Use for any 'how do I X in "
            "Omarchy' question before guessing. Results are grouped by trust tier "
            "(manual > configs > releases > commits). Zero/thin results include an "
            "escalation ladder for finding the answer elsewhere."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string",
                          "description": "Natural language question or keywords"},
                "source_filter": {
                    "type": "string",
                    "enum": ["omarchy", "manual", "config", "release"],
                    "description": "Restrict to a corpus (default: all)"},
                "omarchy_version": {
                    "type": "string",
                    "description": "Only include release notes of this version, e.g. 4.0.0"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 20,
                          "description": "Max results (default 8)"},
            },
            "required": ["query"],
        },
    },
    {
        "name": "omarchy_get_page",
        "description": (
            "Fetch a complete Omarchy manual chapter or developer doc as markdown. "
            "Pass a title (e.g. 'Hotkeys'), a live omarchy.org/manual/... URL, or a "
            "repo path (e.g. manual/06-themes.md). Prefer calling after search."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name_or_url": {"type": "string"},
            },
            "required": ["name_or_url"],
        },
    },
    {
        "name": "omarchy_whats_new",
        "description": (
            "Release notes/changelog. No args = latest release (v4.0.0 Quattro). "
            "version='3.7.0' for a specific release; since='2026-01-01' for every "
            "release after a date."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "version": {"type": "string"},
                "since": {"type": "string", "description": "ISO date, e.g. 2026-06-01"},
            },
        },
    },
    {
        "name": "omarchy_find_config",
        "description": (
            "Find where a setting lives: returns shipped default config file paths "
            "(hypr/*.lua, foot.ini, /etc overrides…), content matches, and manual "
            "references that mention config locations."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "app": {"type": "string",
                        "description": "App or feature, e.g. waybar, hyprland, gtk"},
            },
            "required": ["app"],
        },
    },
    {
        "name": "omarchy_diff_releases",
        "description": (
            "What changed between two Omarchy versions, e.g. from 3.8.4 to 4.0.0 — "
            "the first stop for 'I broke my system with an update'. Use "
            "breaking_only=true to filter to releases flagging breaking changes."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "from_version": {"type": "string", "description": "e.g. 3.8.4"},
                "to_version": {"type": "string", "description": "e.g. 4.0.0"},
                "breaking_only": {"type": "boolean"},
            },
            "required": ["from_version", "to_version"],
        },
    },
    {
        "name": "omarchy_check_updates",
        "description": (
            "Check whether this KB snapshot is behind upstream Omarchy (one GitHub "
            "API call, only when invoked). Returns OUTDATED + the refresh command "
            "when a newer release exists. Call before debugging version-sensitive "
            "behavior or when answers seem to miss recent features."),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "omarchy_kb_info",
        "description": "Knowledge base stats: sources, chunk counts, pinned versions.",
        "inputSchema": {"type": "object", "properties": {}},
    },
]


def slim(rec: dict) -> dict:
    body_cap = 1600
    out = {
        "source": rec["source"],
        "title": rec["title"],
        "section": rec["section"],
        "url": rec["url"],
        "match": rec.get("match"),
        "snippet": rec["snippet"],
        "body": rec["body"][:body_cap] + ("…" if len(rec["body"]) > body_cap else ""),
    }
    if rec.get("version"):
        out["version"] = rec["version"]
    if rec.get("published"):
        out["published"] = rec["published"]
    if rec.get("path"):
        out["path"] = rec["path"]
    return out


def make_result(payload) -> dict:
    return {
        "content": [{"type": "text", "text": json.dumps(payload, indent=1)}],
    }


def handle_call(kb: KnowledgeBase, name: str, args: dict):
    if name == "omarchy_search_docs":
        res = kb.search(
            args["query"],
            source_filter=args.get("source_filter"),
            omarchy_version=args.get("omarchy_version"),
            limit=int(args.get("limit", 8)),
        )
        payload = {"query": args["query"], "results": [slim(r) for r in res]}
        if len(payload["results"]) < 2:
            tokens = tokenize(args["query"])
            payload["did_you_mean"] = suggest_terms(kb.con, tokens)
            payload["escalation"] = ESCALATION
        return payload
    if name == "omarchy_get_page":
        return kb.get_page(args["name_or_url"])
    if name == "omarchy_whats_new":
        return kb.whats_new(version=args.get("version"), since=args.get("since"))
    if name == "omarchy_find_config":
        return kb.find_config(args["app"])
    if name == "omarchy_diff_releases":
        return kb.diff_releases(args["from_version"], args["to_version"],
                                breaking_only=bool(args.get("breaking_only")))
    if name == "omarchy_check_updates":
        return kb.check_updates()
    if name == "omarchy_kb_info":
        m = kb.meta()
        info = {
            "server": SERVER_NAME, "version": SERVER_VERSION,
            "chunks": kb.counts(), "latest_release": m.get("latest_release"),
            "built_from_commit": m.get("built_from_commit"),
            "built_at": m.get("built_at", "unknown"),
            "snapshot_note": (
                "Point-in-time snapshot. Run omarchy_check_updates (network) to "
                "compare against upstream, or just refresh: "
                "scripts/fetch_sources.sh && python3 scripts/build_kb.py"),
            "corpus": "omarchy.org Quattro manual + basecamp/omarchy repo docs, defaults, releases & git history",
        }
        return info
    raise ValueError(f"Unknown tool {name}")


def rpc_error(id_, code: int, message: str) -> dict:
    return {"jsonrpc": "2.0", "id": id_,
            "error": {"code": code, "message": message}}


def main() -> None:
    kb = KnowledgeBase(DB_PATH)
    log(f"ready: {sum(kb.counts().values())} chunks from {DB_PATH}")

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            print(json.dumps(rpc_error(None, -32700, f"Parse error: {e}")),
                  flush=True)
            continue

        method = req.get("method", "")
        id_ = req.get("id")

        if method == "initialize":
            client_proto = (req.get("params") or {}).get("protocolVersion")
            proto = client_proto or "2024-11-05"
            print(json.dumps({
                "jsonrpc": "2.0", "id": id_,
                "result": {
                    "protocolVersion": proto,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                },
            }), flush=True)
        elif method == "notifications/initialized":
            continue  # notification: no response
        elif method == "ping":
            print(json.dumps({"jsonrpc": "2.0", "id": id_, "result": {}}),
                  flush=True)
        elif method == "tools/list":
            print(json.dumps({"jsonrpc": "2.0", "id": id_,
                              "result": {"tools": TOOLS}}), flush=True)
        elif method == "tools/call":
            params = req.get("params") or {}
            try:
                payload = handle_call(kb, params.get("name", ""),
                                      params.get("arguments") or {})
                print(json.dumps({"jsonrpc": "2.0", "id": id_,
                                  "result": make_result(payload)}), flush=True)
            except Exception as e:  # noqa: BLE001 — report as tool error
                log(f"tool error: {e}")
                print(json.dumps({"jsonrpc": "2.0", "id": id_, "result": {
                    "content": [{"type": "text", "text": f"Error: {e}"}],
                    "isError": True}}), flush=True)
        elif id_ is not None:
            print(json.dumps(rpc_error(id_, -32601,
                                       f"Method not found: {method}")), flush=True)


if __name__ == "__main__":
    main()
