#!/usr/bin/env python3
"""Build the SQLite knowledge base from raw Omarchy sources.

Corpus (all official):
  - manual:    manual/NN-*.md from the basecamp/omarchy repo (Quattro v4 source)
  - docs:      repo-internal developer docs (docs/*.md)
  - configs:   shipped default configs (config/**, default/**) as text files
  - releases:  GitHub release notes (data/raw/releases.json)

Output: data/kb.sqlite3 with an FTS5 table + metadata. Rerunning rebuilds it.
"""
import json
import re
import sqlite3
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RAW = ROOT / "data" / "raw"
REPO = RAW / "omarchy-repo"
DB = ROOT / "data" / "kb.sqlite3"

# Manual chapter -> live URL mapping is derived from the official site search
# index (chapter title -> url), matched against each repo file's H1 heading.
SEARCH_INDEX_URL = "https://omarchy.org/manual/search-index.json"
SEARCH_INDEX_FILE = RAW / "manual-search-index.json"


def _norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]+", "", s.lower())


def load_url_map():
    """Return {normalized chapter title: absolute omarchy.org URL}."""
    try:
        idx = json.loads(SEARCH_INDEX_FILE.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}
    out = {}
    for e in idx:
        out.setdefault(_norm(e["chapter"]), f"https://omarchy.org{e['url']}")
    return out


CONFIG_EXTS = {".conf", ".ini", ".toml", ".kdl", ".css", ".rasi", ".json",
               ".sh", ".bash", ".zsh", ".lua"}
CONFIG_SKIP_DIRS = {"applications", "autostart", "environment.d", "fontconfig", "systemd", "wallpapers", " backgrounds"}


def clean_markdown(text: str) -> str:
    """Strip image lines and collapse whitespace; keep code fences intact."""
    text = re.sub(r"^!\[[^\]]*\]\([^)]*\)\s*$", "", text, flags=re.M)
    text = re.sub(r"\n{3,}", "\n\n", text)
    return text.strip()


def split_sections(md_text: str, fallback_title: str):
    """Split a markdown doc into (heading, body) sections at ## level.

    Returns a list of dicts with title and content; the preamble becomes
    one section titled after the file's # heading or fallback_title."""
    lines = md_text.splitlines()
    sections = []
    cur_title = fallback_title
    cur = []

    def flush():
        body = "\n".join(cur).strip()
        if body:
            sections.append((cur_title, body))

    for line in lines:
        m = re.match(r"^##\s+(.*)$", line)
        if m:
            flush()
            cur = []
            cur_title = m.group(1).strip()
        else:
            cur.append(line)
    flush()
    return sections


def add_doc(cur, *, source, title, section, body, path, url, version=None,
            published=None, priority=1):
    cur.execute(
        "INSERT INTO chunks(source, title, section, body, path, url, version, published, priority) "
        "VALUES (?,?,?,?,?,?,?,?,?)",
        (source, title, section, body, path, url, version, published, priority),
    )


def ingest_manual(cur):
    url_map = load_url_map()
    count = 0
    manual_dir = REPO / "manual"
    for f in sorted(manual_dir.glob("*.md")):
        text = clean_markdown(f.read_text(encoding="utf-8-sig"))
        h1 = re.search(r"^#\s+(.+)$", text, re.M)
        title = h1.group(1).strip() if h1 else f.stem
        url = url_map.get(_norm(title))
        if not url:
            print(f"  ! no live URL for {f.name} ({title!r}); indexing without link")
        for sec_title, body in split_sections(text, title):
            add_doc(cur, source="manual", title=title, section=sec_title,
                    body=f"# {sec_title}\n\n{body}" if sec_title != title else body,
                    path=str(f.relative_to(REPO)), url=url or "")
            count += 1
    return count


def ingest_repo_docs(cur):
    count = 0
    for f in sorted((REPO / "docs").glob("*.md")):
        text = clean_markdown(f.read_text(encoding="utf-8"))
        h1 = re.match(r"^#\s+(.*)$", text)
        title = h1.group(1).strip() if h1 else f.stem
        for sec_title, body in split_sections(text, title):
            add_doc(cur, source="repo-docs", title=title, section=sec_title,
                    body=body, path=str(f.relative_to(REPO)),
                    url=f"https://github.com/basecamp/omarchy/tree/master/{f.relative_to(REPO)}")
            count += 1
    return count


def ingest_configs(cur):
    """Index every default config file as a single chunk keyed by its path."""
    count = 0
    for base in ("config", "default", "etc"):
        basedir = REPO / base
        if not basedir.exists():
            continue
        for f in sorted(basedir.rglob("*")):
            if not f.is_file():
                continue
            rel = f.relative_to(REPO)
            parts = rel.parts
            if any(s.strip() in CONFIG_SKIP_DIRS for s in parts):
                continue
            # Skip binaries and huge files
            if f.stat().st_size > 200_000:
                continue
            try:
                head = f.read_bytes()[:4096]
                head.decode("utf-8")
            except UnicodeDecodeError:
                continue
            ext = f.suffix.lower()
            name = f.name.lower()
            if ext not in CONFIG_EXTS and name not in {
                "hyprland.conf", "hyprlock.conf", "hypridle.conf", "makefile",
            } and not name.endswith(".conf"):
                continue
            try:
                body = f.read_text(encoding="utf-8").strip()
            except UnicodeDecodeError:
                continue
            if not body:
                continue
            add_doc(cur, source="config", title=str(rel),
                    section="", body=body[:50_000], path=str(rel), url="",
                    priority=2)
            count += 1
    return count


def split_release_sections(markdown: str):
    sections = []
    cur_title = "Overview"
    cur = []
    for line in markdown.splitlines():
        if line.startswith("## "):
            if cur:
                sections.append((cur_title, "\n".join(cur).strip()))
            cur = []
            cur_title = line[3:].strip()
        else:
            cur.append(line)
    if cur:
        sections.append((cur_title, "\n".join(cur).strip()))
    return sections


def ingest_commits(cur):
    """One chunk per non-merge commit: DHH's design rationale lives here."""
    import subprocess

    count = 0
    try:
        raw = subprocess.run(
            ["git", "-C", str(REPO), "log", "--no-merges",
             "--format=%H%x00%as%x00%s%x00%b%x01"],
            capture_output=True, text=True, check=True).stdout
    except (subprocess.CalledProcessError, FileNotFoundError):
        print("  ! git history unavailable; skipping commit index")
        return 0
    for record in raw.split("\x01"):
        record = record.strip("\n")
        if not record:
            continue
        parts = record.split("\x00", 3)
        if len(parts) < 3:
            continue
        sha, date, subject = parts[0], parts[1], parts[2]
        body = parts[3].strip() if len(parts) == 4 else ""
        content = subject if not body else f"{subject}\n\n{body}"
        add_doc(cur, source="commit",
                title=f"commit {sha[:12]}: {subject}",
                section="", body=content[:50_000],
                path=sha,
                url=f"https://github.com/basecamp/omarchy/commit/{sha}",
                published=date)
        count += 1
    return count


def ingest_releases(cur):
    count = 0
    rel_file = RAW / "releases.json"
    if not rel_file.exists():
        return 0
    releases = json.loads(rel_file.read_text(encoding="utf-8"))
    for r in releases:
        version = (r.get("tag_name") or "").lstrip("v")
        body = r.get("body") or ""
        if not version or not body.strip():
            continue
        published = (r.get("published_at") or "")[:10]
        for sec_title, sec_body in split_release_sections(body):
            if len(sec_body.split()) < 5:
                continue
            add_doc(cur, source="release", title=f"Omarchy v{version}",
                    section=sec_title, body=sec_body, path="",
                    url=r.get("html_url", ""), version=version,
                    published=published)
            count += 1
    return count


def main():
    if not REPO.exists():
        sys.exit("Run scripts/fetch_sources.sh first.")
    DB.parent.mkdir(parents=True, exist_ok=True)
    if DB.exists():
        DB.unlink()

    con = sqlite3.connect(DB)
    cur = con.cursor()
    cur.execute("""
        CREATE TABLE chunks(
            id INTEGER PRIMARY KEY,
            source TEXT NOT NULL,
            title TEXT NOT NULL,
            section TEXT DEFAULT '',
            body TEXT NOT NULL,
            path TEXT DEFAULT '',
            url TEXT DEFAULT '',
            version TEXT,
            published TEXT,
            priority INTEGER DEFAULT 1
        )
    """)
    cur.execute("""
        CREATE VIRTUAL TABLE chunks_fts USING fts5(
            title, section, body, content='chunks', content_rowid='id',
            tokenize='porter unicode61'
        )
    """)
    cur.execute("""
        CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
            INSERT INTO chunks_fts(rowid, title, section, body)
            VALUES (new.id, new.title, new.section, new.body);
        END
    """)
    cur.execute("""
        CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
            INSERT INTO chunks_fts(chunks_fts, rowid, title, section, body)
            VALUES ('delete', old.id, old.title, old.section, old.body);
        END
    """)

    n_manual = ingest_manual(cur)
    n_docs = ingest_repo_docs(cur)
    n_conf = ingest_configs(cur)
    n_rel = ingest_releases(cur)
    n_commits = ingest_commits(cur)

    meta_version = "unknown"
    rf = RAW / "releases.json"
    if rf.exists():
        rels = json.loads(rf.read_text())
        if rels:
            meta_version = rels[0].get("tag_name", "unknown")
    cur.execute("""
        CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT)""")
    con.executemany("INSERT OR REPLACE INTO meta VALUES (?,?)", [
        ("latest_release", meta_version),
        ("built_from_commit", _repo_head()),
    ])

    # Vocabulary table for "did you mean" suggestions on zero-result queries.
    cur.execute("""
        CREATE VIRTUAL TABLE IF NOT EXISTS terms USING fts5vocab(chunks_fts, 'row')""")

    counts = dict(cur.execute(
        "SELECT source, COUNT(*) FROM chunks GROUP BY source").fetchall())
    print(f"Built {DB.name}: {sum(counts.values())} chunks {counts}")
    print(f"Latest release pinned: {meta_version}")
    con.commit()
    con.close()


def _repo_head():
    git = REPO / ".git" / "HEAD"
    if not git.exists():
        return "unknown"
    ref = git.read_text().strip().split(": ")
    if len(ref) == 2:
        return (REPO / ".git" / ref[1]).read_text().strip()[:12]
    return ref[0][:12]


if __name__ == "__main__":
    main()
