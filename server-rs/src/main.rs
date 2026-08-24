//! mcp-server-omarchy — offline Omarchy (Quattro/v4) documentation MCP server.
//!
//! Rust port of `server/mcp_server.py`: newline-delimited JSON-RPC over stdio
//! per the MCP spec, backed by the same SQLite FTS5 knowledge base
//! (`data/kb.sqlite3`). The database is read-only at runtime; rebuild it with
//! `scripts/build_kb.py`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use rusqlite::types::Value as SqlValue;
use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Map, Value};

const SERVER_NAME: &str = "omarchy-docs";
const SERVER_VERSION: &str = "1.0.0";

/// Result tiers: the current manual beats repo internals beats shipped configs
/// beats historical release notes, regardless of raw bm25 score.
const TIERS: &[(&str, &[&str])] = &[
    ("core", &["manual", "repo-docs"]),
    ("configs", &["config"]),
    ("releases", &["release"]),
    ("history", &["commit"]),
];

fn tier_limit(tier: &str) -> usize {
    match tier {
        "core" => 6,
        "configs" => 3,
        "releases" => 4,
        _ => 3,
    }
}

const STOPWORDS: &[&str] = &[
    "how", "do", "does", "i", "a", "an", "the", "to", "in", "on", "for", "of", "my",
    "me", "is", "are", "it", "its", "what", "whats", "when", "where", "which", "can",
    "with", "and", "or", "get", "got",
];

const RELEASE_CAP: usize = 40_000;
const BODY_CAP: usize = 1600;

const RELEASES_API: &str =
    "https://api.github.com/repos/basecamp/omarchy/releases?per_page=5";

const ESCALATION_RETRY_HINTS: &[&str] = &[
    "Use Omarchy's exact vocabulary: quickshell, keybind, theme, snapshot, factory reset",
    "Try one distinctive noun instead of a full sentence",
    "Config/keybind/default question? Use omarchy_find_config",
    "'When did X change?' questions belong in omarchy_whats_new",
];

const ESCALATION_NEXT_STEPS: &str = "Thin results may mean the topic is undocumented. Escalate: \
     (1) read the source at github.com/basecamp/omarchy — bin/ scripts and \
     git log are the real docs for internals; \
     (2) upstream docs — quickshell.org/docs (v4 shell), wiki.hypr.land \
     (window manager options), wiki.archlinux.org (everything below); \
     (3) search closed issues at github.com/basecamp/omarchy/issues; \
     (4) ask on the Omarchy Discord discord.gg/tXFUdasqhY with omarchy-debug output attached. \
     Never invent Omarchy-specific behavior from general Arch/Hyprland knowledge; \
     if unverified, say so.";

// --------------------------------------------------------------------- setup

fn log(msg: &str) {
    eprintln!("[{SERVER_NAME}] {msg}");
}

/// Resolve the project root (the directory holding data/kb.sqlite3).
/// Preference order: $OMARCHY_KB_ROOT, --kb <dir>, the binary's own directory,
/// then the compile-time path of this crate's parent (the Python layout).
fn project_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OMARCHY_KB_ROOT") {
        let p = PathBuf::from(p);
        if p.join("data/kb.sqlite3").exists() {
            return Some(p);
        }
    }
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--kb" {
            if let Some(p) = args.next() {
                let p = PathBuf::from(p);
                if p.join("data/kb.sqlite3").exists() {
                    return Some(p);
                }
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|p| p.parent()) {
            if root.join("data/kb.sqlite3").exists() {
                return Some(root.to_path_buf());
            }
        }
    }
    // Fallback for `cargo run` / target-dir binaries inside the original checkout.
    let manifest_parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .to_path_buf();
    if manifest_parent.join("data/kb.sqlite3").exists() {
        return Some(manifest_parent);
    }
    None
}

fn load_json(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

// -------------------------------------------------------------------- lexing

fn tokenize(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '+' || c == '#' || c == '.' || c == '-'))
        .filter(|t| !t.is_empty() && !STOPWORDS.contains(t))
        .map(str::to_owned)
        .collect()
}

/// Quote a token for FTS5 so operators/punctuation stay literal.
fn fts_escape(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

fn expand_synonyms(tokens: &[String], synonyms: &Map<String, Value>) -> Vec<String> {
    let mut extra: Vec<String> = Vec::new();
    for t in tokens {
        if let Some(Value::Array(alts)) = synonyms.get(t) {
            for alt in alts {
                if let Some(alt) = alt.as_str() {
                    for w in alt.split_whitespace() {
                        if !tokens.iter().any(|x| x == w) && !extra.iter().any(|x| x == w) {
                            extra.push(w.to_string());
                        }
                    }
                }
            }
        }
    }
    extra
}

/// Nearest vocabulary terms for zero-result queries ("did you mean").
fn suggest_terms(con: &Connection, tokens: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tokens {
        if t.len() < 4 || t.ends_with('*') {
            continue;
        }
        let prefix_len = std::cmp::max(4, t.len().saturating_sub(2));
        let prefix: String = t.chars().take(prefix_len).collect();
        let mut stmt = match con.prepare_cached(
            "SELECT DISTINCT term FROM terms WHERE term LIKE ?1 || '%' LIMIT 3",
        ) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let rows = stmt
            .query_map([prefix], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(Result::ok).collect::<Vec<_>>())
            .unwrap_or_default();
        for term in rows {
            if term != *t && !out.contains(&term) {
                out.push(term);
            }
        }
        if out.len() >= 5 {
            break;
        }
    }
    out.truncate(5);
    out
}

// ----------------------------------------------------------------- knowledge

struct KnowledgeBase {
    con: Connection,
    synonyms: Map<String, Value>,
    upstream_map: Map<String, Value>,
}

/// One search hit before response slimming. Mirrors the Python dict shape.
#[derive(Clone)]
struct Rec {
    source: String,
    title: String,
    section: String,
    body: String,
    url: String,
    path: String,
    version: Option<String>,
    published: Option<String>,
    snippet: String,
    tier: &'static str,
    exactness: &'static str,
    see_also: Vec<String>,
}

impl KnowledgeBase {
    fn open(db_path: &Path) -> rusqlite::Result<Self> {
        let con = Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        Ok(Self {
            con,
            synonyms: load_json(&db_path.with_file_name("synonyms.json")),
            upstream_map: load_json(&db_path.with_file_name("upstream_map.json")),
        })
    }

    /// SELECT key, value FROM meta as a map.
    fn meta_map(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        if let Ok(mut stmt) = self.con.prepare("SELECT key, value FROM meta") {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    m.insert(row.0, row.1);
                }
            }
        }
        m
    }

    fn counts(&self) -> Vec<(String, i64)> {
        let mut stmt = match self.con.prepare("SELECT source, COUNT(*) FROM chunks GROUP BY source") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    fn total_chunks(&self) -> i64 {
        self.counts().iter().map(|(_, n)| n).sum()
    }

    fn _run_match(
        &self,
        match_expr: &str,
        sources: &[&str],
        limit: usize,
    ) -> Vec<Rec> {
        let placeholders: Vec<&str> = sources.iter().map(|_| "?").collect();
        let sql = format!(
            "SELECT c.source, c.title, c.section, c.body, c.url, c.path, \
                    c.version, c.published, \
                    bm25(chunks_fts, 6.0, 3.0, 1.0) AS rank, \
                    highlight(chunks_fts, 2, '<<', '>>') AS snippet \
             FROM chunks_fts f JOIN chunks c ON c.id = f.rowid \
             WHERE chunks_fts MATCH ? AND c.source IN ({}) \
             ORDER BY rank LIMIT {}",
            placeholders.join(","),
            limit
        );
        let mut params: Vec<SqlValue> = vec![SqlValue::from(match_expr.to_string())];
        params.extend(sources.iter().map(|s| SqlValue::from(s.to_string())));
        let mut stmt = match self.con.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(params_from_iter(params.into_iter()), |r| {
            Ok(Rec {
                source: r.get("source")?,
                title: r.get("title")?,
                section: r.get::<_, Option<String>>("section")?.unwrap_or_default(),
                body: r.get("body")?,
                url: r.get::<_, Option<String>>("url")?.unwrap_or_default(),
                path: r.get::<_, Option<String>>("path")?.unwrap_or_default(),
                version: r.get("version")?,
                published: r.get("published")?,
                snippet: r.get("snippet")?,
                tier: "",
                exactness: "",
                see_also: Vec::new(),
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
    }

    fn search(
        &self,
        query: &str,
        source_filter: Option<&str>,
        omarchy_version: Option<&str>,
        limit: usize,
    ) -> Vec<Rec> {
        let tokens = tokenize(query);
        if tokens.is_empty() {
            return Vec::new();
        }

        let tiers: &[(&str, &[&str])] = match source_filter {
            Some("omarchy") => &[("core", &["manual", "repo-docs"]), ("releases", &["release"])],
            Some("manual") | Some("docs") => &[TIERS[0]],
            Some("config") => &[TIERS[1]],
            Some("release") => &[TIERS[2]],
            _ => TIERS,
        };

        let synonyms = expand_synonyms(&tokens, &self.synonyms);
        let and_expr = tokens
            .iter()
            .map(|t| format!("{}*", fts_escape(t)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let or_terms: Vec<&String> = tokens
            .iter()
            .chain(
                synonyms
                    .iter()
                    .filter(|s| !tokens.iter().any(|b| s.starts_with(b.as_str()) || b.starts_with(s.as_str()))),
            )
            .collect();
        let or_expr = or_terms
            .iter()
            .map(|t| format!("{}*", fts_escape(t)))
            .collect::<Vec<_>>()
            .join(" OR ");

        // Best-of-each-tier in priority order (manual > configs > releases > commits).
        // Like the Python original, each pass starts from a fresh seen-set; the
        // OR pass is deduplicated against the AND pass only afterwards.
        let collect = |expr: &str, exactness: &'static str| -> Vec<Rec> {
            let mut results: Vec<Rec> = Vec::new();
            let mut seen: HashSet<(String, String, String)> = HashSet::new();
            for &(tier_name, sources) in tiers {
                let cap = tier_limit(tier_name);
                let mut taken = 0usize;
                for row in self._run_match(expr, sources, cap * 2 + limit) {
                    let key = (
                        row.source.clone(),
                        row.title.clone(),
                        row.section.clone(),
                    );
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.insert(key);
                    let mut rec = row;
                    rec.tier = tier_name;
                    rec.exactness = exactness;
                    results.push(rec);
                    taken += 1;
                    if taken >= cap {
                        break;
                    }
                }
            }
            results
        };

        let mut out: Vec<Rec> = collect(&and_expr, "exact");
        if out.len() < limit {
            let mut seen_keys: HashSet<(String, String, String)> = out
                .iter()
                .map(|r| (r.source.clone(), r.title.clone(), r.section.clone()))
                .collect();
            for rec in collect(&or_expr, "partial") {
                let key = (rec.source.clone(), rec.title.clone(), rec.section.clone());
                if !seen_keys.contains(&key) {
                    seen_keys.insert(key);
                    out.push(rec);
                }
            }
        }

        // Tag results with curated upstream links when the query matches.
        let q_lower = query.to_lowercase();
        let see_also: Vec<String> = self
            .upstream_map
            .iter()
            .filter_map(|(phrase, url)| {
                let url = url.as_str()?;
                phrase
                    .split_whitespace()
                    .all(|w| q_lower.contains(w))
                    .then(|| url.to_string())
            })
            .take(3)
            .collect();
        if !see_also.is_empty() {
            for rec in out.iter_mut() {
                rec.see_also = see_also.clone();
            }
        }

        out.truncate(limit);

        if let Some(v) = omarchy_version {
            let wanted = v.trim_start_matches('v');
            out.retain(|r| {
                r.source != "release"
                    || r.version
                        .as_deref()
                        .map(|rv| rv.trim_start_matches('v') == wanted)
                        .unwrap_or(false)
            });
        }
        out
    }

    // ---------------------------------------------------------------- pages

    fn get_page(&self, name_or_url: &str) -> Value {
        let q = name_or_url.trim();
        let norm_q: String = norm_key(q);

        if q.starts_with("http://") || q.starts_with("https://") {
            let title: Option<String> = self
                .con
                .query_row(
                    "SELECT DISTINCT title FROM chunks WHERE url = ?1 LIMIT 1",
                    [q],
                    |r| r.get(0),
                )
                .ok();
            if let Some(title) = title {
                return self.page_by_title(&title);
            }
        }
        if q.contains('/') || q.ends_with(".md") {
            let titles: Vec<String> = self.titles_matching_path(q);
            return match titles.len() {
                0 => json!({ "error": format!("No page matching '{}'", q.replace('\'', "''")) }),
                1 => self.page_by_title(&titles[0]),
                _ => json!({ "error": "ambiguous", "candidates": titles }),
            };
        }

        // By (partial) title across manual + repo docs
        let mut matches: Vec<String> = Vec::new();
        if let Ok(mut stmt) = self
            .con
            .prepare("SELECT DISTINCT title FROM chunks WHERE source IN ('manual','repo-docs')")
        {
            if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(0)) {
                for t in rows.flatten() {
                    let norm_t = norm_key(&t);
                    if !norm_q.is_empty()
                        && (norm_q == norm_t || norm_t.contains(&norm_q) || norm_t.starts_with(&norm_q))
                    {
                        matches.push(t);
                    }
                }
            }
        }
        match matches.len() {
            0 => json!({
                "error": format!("No page titled '{}'", name_or_url.replace('\'', "''")),
                "hint": "Use omarchy_search_docs first, then pass result.title or .url"
            }),
            1 => self.page_by_title(&matches.remove(0)),
            _ => {
                matches.sort();
                json!({ "error": "ambiguous", "candidates": matches })
            }
        }
    }

    fn titles_matching_path(&self, q: &str) -> Vec<String> {
        let like = format!("%{q}%");
        let mut stmt = match self
            .con
            .prepare("SELECT DISTINCT title FROM chunks WHERE path LIKE ?1 ORDER BY title")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([like], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    fn page_by_title(&self, title: &str) -> Value {
        let mut stmt = match self.con.prepare(
            "SELECT source, section, body, url, path FROM chunks \
             WHERE title = ?1 AND source IN ('manual','repo-docs') ORDER BY id",
        ) {
            Ok(s) => s,
            Err(_) => return json!({ "error": format!("No page titled '{}'", title) }),
        };
        let rows: Vec<(String, String, String, String, String)> = stmt
            .query_map([title], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                ))
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default();
        if rows.is_empty() {
            return json!({ "error": format!("No page titled '{}'", title) });
        }
        let mut parts: Vec<String> = Vec::new();
        let (mut source, mut url, mut path) = (String::new(), String::new(), String::new());
        for (src, section, body, u, p) in rows {
            source = src;
            url = u;
            path = p;
            if !section.is_empty() && body.starts_with('#') {
                parts.push(body);
            } else if !section.is_empty() {
                parts.push(format!("## {section}\n\n{body}"));
            } else {
                parts.push(body);
            }
        }
        let text = parts.join("\n\n");
        json!({
            "source": source,
            "title": title,
            "path": path,
            "url": url,
            "chars": text.chars().count(),
            "markdown": text,
        })
    }

    // ------------------------------------------------------------- releases

    fn release_rows_for_version(&self, version: &str) -> Vec<ReleaseRow> {
        let mut stmt = match self.con.prepare(
            "SELECT version, published, section, body, url FROM chunks \
             WHERE source='release' AND version=?1 ORDER BY id",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([version], map_release_row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    fn all_release_rows(&self) -> Vec<ReleaseRow> {
        let mut stmt = match self.con.prepare(
            "SELECT version, published, section, body, url FROM chunks \
             WHERE source='release' ORDER BY published DESC, id",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], map_release_row)
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    fn whats_new(&self, version: Option<&str>, since: Option<&str>) -> Value {
        if let Some(version) = version {
            let v = version.trim_start_matches('v');
            let no_dots = v.replace('.', "");
            // LIKE '%40%' would also match 4.10.x etc.; filter precisely after.
            let pattern = format!("%{no_dots}%");
            let mut stmt = match self.con.prepare(
                "SELECT version, published, section, body, url FROM chunks \
                 WHERE source='release' AND REPLACE(version,'.','') LIKE ?1 \
                 ORDER BY published DESC, id",
            ) {
                Ok(s) => s,
                Err(_) => return json!({ "error": format!("No release '{}'", version) }),
            };
            let mut rows: Vec<ReleaseRow> = stmt
                .query_map([&pattern], map_release_row)
                .map(|rs| rs.filter_map(Result::ok).collect())
                .unwrap_or_default();
            rows.retain(|r| r.version.trim_start_matches('v') == v);
            if rows.is_empty() {
                let mut avail: Vec<String> = {
                    let all_rows = self.all_release_rows();
                    let versions: HashSet<String> =
                        all_rows.into_iter().map(|r| r.version).collect();
                    versions.into_iter().collect()
                };
                avail.sort_by(|a, b| cmp_versions(b, a));
                let avail: Vec<String> = avail.into_iter().take(15).collect();
                return json!({ "error": format!("No release '{}'", version), "available": avail });
            }
            return format_release(&rows[0].version.clone(), &rows);
        }

        if let Some(since) = since {
            let versions = self.release_versions_since(since);
            if versions.is_empty() {
                return json!({ "error": format!("No releases after {since}") });
            }
            let all_rows = self.all_release_rows();
            let mut releases = Vec::new();
            for v in versions {
                let vr: Vec<ReleaseRow> = all_rows.iter().filter(|r| r.version == v).cloned().collect();
                releases.push(format_release(&v, &vr));
            }
            return json!({ "count": releases.len(), "releases": releases });
        }

        // Latest release overall.
        let latest = self.latest_release_version();
        match latest {
            Some(v) if !v.is_empty() => {
                let rows = self.release_rows_for_version(&v);
                format_release(&v, &rows)
            }
            _ => json!({ "error": "No release notes indexed" }),
        }
    }

    fn latest_release_version(&self) -> Option<String> {
        self.con
            .query_row(
                "SELECT version FROM chunks WHERE source='release' \
                 ORDER BY published DESC, id LIMIT 1",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten()
    }

    fn release_versions_since(&self, since: &str) -> Vec<String> {
        let mut stmt = match self.con.prepare(
            "SELECT version, MIN(id) AS min_id FROM chunks \
             WHERE source='release' AND published > ?1 \
             GROUP BY version ORDER BY published DESC, min_id",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([since], |r| r.get::<_, String>(0))
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
    }

    fn diff_releases(&self, from_version: &str, to_version: &str, breaking_only: bool) -> Value {
        let known = self.known_versions_ascending();
        let strip = |v: &str| v.trim_start_matches('v').to_string();
        let f = strip(from_version);
        let t = strip(to_version);
        let has = |v: &str| known.iter().any(|k| k == v);
        if !has(&f) && !has(&format!("v{f}")) {
            return json!({
                "error": format!("Unknown version '{}'", from_version),
                "known": last_n(&known, 10),
            });
        }
        if !has(&t) && !has(&format!("v{t}")) {
            return json!({
                "error": format!("Unknown version '{}'", to_version),
                "known": last_n(&known, 10),
            });
        }

        let key = parse_version_key;
        let lo = key(&f);
        let hi = key(&t);
        let between: Vec<String> = known
            .iter()
            .filter(|v| {
                let bare = v.trim_start_matches('v').to_string();
                bare != f && lt_versions(&lo, &key(&bare)) && le_versions(&key(&bare), &hi)
            })
            .cloned()
            .collect();
        if between.is_empty() {
            return json!({
                "error": format!("No releases between {f} and {t}"),
                "hint": "They may be the same or in the wrong order"
            });
        }

        let all_rows = self.all_release_rows();
        let mut releases: Vec<Value> = Vec::new();
        for v in &between {
            let vr: Vec<ReleaseRow> =
                all_rows.iter().filter(|r| r.version == *v).cloned().collect();
            let rel = format_release(v, &vr);
            if breaking_only {
                let text = rel["notes"].as_str().unwrap_or("").to_lowercase();
                let flagged = ["breaking", "removed", "renamed", "migration", "moved to", "now requires", "incompatible"]
                    .iter()
                    .any(|m| text.contains(m));
                if !flagged {
                    continue;
                }
            }
            releases.push(rel);
        }
        if breaking_only && releases.is_empty() {
            return json!({
                "from": f, "to": t, "releases": [],
                "note": "No release in this range flagged breaking changes by keyword; call without breaking_only to see all."
            });
        }
        json!({ "from": f, "to": t, "count": releases.len(), "releases": releases })
    }

    fn known_versions_ascending(&self) -> Vec<String> {
        let mut stmt = match self.con.prepare(
            "SELECT DISTINCT version FROM chunks WHERE source='release' ORDER BY published",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |r| r.get::<_, Option<String>>(0))
            .map(|rows| {
                rows.filter_map(Result::ok)
                    .filter_map(|o| o)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    // -------------------------------------------------------------- updates

    fn check_updates(&self) -> Value {
        // Compare the KB snapshot against upstream. Network call on invoke.
        let m = self.meta_map();
        let local_latest = m
            .get("latest_release")
            .map(|s| s.trim_start_matches('v').to_string())
            .unwrap_or_default();

        let fetch = fetch_releases_json(RELEASES_API);
        let info = self.snapshot_info();
        let obj = match fetch {
            Err(e) => {
                let mut o = json!({
                    "error": format!("Could not reach GitHub API: {e}"),
                    "manual_check": "https://github.com/basecamp/omarchy/releases",
                });
                if let Some(local) = o.as_object_mut() {
                    if let Value::Object(map) = info {
                        local.insert("local".to_string(), Value::Object(map));
                    }
                }
                o
            }
            Ok(releases) => {
                let upstream: Vec<String> = releases
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|r| r.get("tag_name").and_then(|t| t.as_str()))
                            .filter(|t| !t.is_empty())
                            .map(|t| t.trim_start_matches('v').to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let newest = upstream.first().cloned();
                let differs = matches!(&newest, Some(n) if *n != local_latest)
                    && !local_latest.is_empty();
                let mut merged = info;
                if differs {
                    let newest = newest.as_deref().unwrap_or_default();
                    let outdated = cmp_versions(newest, &local_latest) == Ordering::Greater;
                    let status = if outdated {
                        "OUTDATED — refresh now".to_string()
                    } else {
                        "KB is ahead of tagged releases (unreleased commits indexed)".to_string()
                    };
                    if let Some(obj) = merged.as_object_mut() {
                        obj.insert("upstream_newest_release".into(), json!(newest));
                        obj.insert("status".into(), Value::String(status));
                        obj.insert(
                            "refresh".into(),
                            json!("scripts/fetch_sources.sh && python3 scripts/build_kb.py"),
                        );
                        obj.insert(
                            "recent_upstream_releases".into(),
                            json!(upstream.iter().take(5).collect::<Vec<_>>()),
                        );
                    }
                } else {
                    let obj = match merged.as_object_mut() {
                        Some(o) => o,
                        None => return json!({}),
                    };
                    obj.insert(
                        "upstream_newest_release".into(),
                        newest.map(Value::String).unwrap_or(Value::Null),
                    );
                    obj.insert("status".into(), json!("UP TO DATE"));
                }
                merged
            }
        };
        obj
    }

    fn snapshot_info(&self) -> Value {
        let m = self.meta_map();
        json!({
            "local_snapshot": {
                "latest_release": m.get("latest_release").cloned().map(Value::String).unwrap_or(Value::Null),
                "built_from_commit": m.get("built_from_commit").cloned().map(Value::String).unwrap_or(Value::Null),
                "built_at": m.get("built_at").cloned().unwrap_or_else(|| "unknown (rebuild to stamp)".into()),
                "chunks": self.total_chunks(),
            }
        })
    }

    // ---------------------------------------------------------- find config

    fn find_config(&self, app: &str) -> Value {
        let like = format!("%{app}%");
        let mut by_path_stmt = match self.con.prepare(
            "SELECT path, body FROM chunks WHERE source='config' \
               AND (path LIKE ?1 OR title LIKE ?1) \
             ORDER BY LENGTH(path) LIMIT 12",
        ) {
            Ok(s) => s,
            Err(_) => return json!({ "app": app }),
        };
        let config_files_by_path: Vec<Value> = by_path_stmt
            .query_map([&like], |r| {
                Ok(json!({
                    "path": r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    "excerpt": truncate_chars(&r.get::<_, String>(1)?, 600),
                }))
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default();

        let content_hits = self.search(app, Some("config"), None, 6);
        let config_files_by_content: Vec<Value> = content_hits
            .iter()
            .map(|rec| {
                json!({
                    "path": rec.path,
                    "snippet": truncate_chars(&rec.snippet, 600),
                })
            })
            .collect();

        let doc_references: Vec<Value> = {
            let sql = format!(
                "SELECT c.title AS page_title, c.section, c.url, \
                        highlight(chunks_fts, 2, '<<', '>>') AS snippet \
                 FROM chunks_fts f JOIN chunks c ON c.id=f.rowid \
                 WHERE chunks_fts MATCH ?1 AND c.source IN ('manual','repo-docs') \
                   AND (c.body LIKE '%~/.config%' OR c.body LIKE '%/etc/%') \
                 ORDER BY rank LIMIT 6"
            );
            let match_expr = format!("{}*", fts_escape(app));
            let mut stmt = match self.con.prepare(&sql) {
                Ok(s) => s,
                Err(_) => return json!({ "app": app }),
            };
            stmt.query_map([&match_expr], |r| {
                Ok(json!({
                    "page": r.get::<_, String>(0)?,
                    "section": r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    "url": r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    "snippet": r.get::<_, String>(3)?,
                }))
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default()
        };

        json!({
            "app": app,
            "config_files_by_path": config_files_by_path,
            "config_files_by_content": config_files_by_content,
            "doc_references": doc_references,
        })
    }

    fn kb_info(&self) -> Value {
        let m = self.meta_map();
        let chunks: Map<_, _> = self
            .counts()
            .into_iter()
            .map(|(k, v)| (k, Value::from(v)))
            .collect();
        json!({
            "server": SERVER_NAME,
            "version": SERVER_VERSION,
            "chunks": Value::Object(chunks),
            "latest_release": m.get("latest_release").cloned().map(Value::String).unwrap_or(Value::Null),
            "built_from_commit": m.get("built_from_commit").cloned().map(Value::String).unwrap_or(Value::Null),
            "built_at": m.get("built_at").cloned().unwrap_or_else(|| "unknown".into()),
            "snapshot_note": "Point-in-time snapshot. Run omarchy_check_updates (network) to compare against upstream, or just refresh: scripts/fetch_sources.sh && python3 scripts/build_kb.py",
            "corpus": "omarchy.org Quattro manual + basecamp/omarchy repo docs, defaults, releases & git history",
        })
    }
}

// ------------------------------------------------------------- release rows

#[derive(Clone)]
struct ReleaseRow {
    version: String,
    #[allow(dead_code)]
    published: Option<String>,
    section: String,
    body: String,
    #[allow(dead_code)]
    url: Option<String>,
}

fn map_release_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReleaseRow> {
    Ok(ReleaseRow {
        version: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
        published: r.get(1)?,
        section: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        body: r.get(3)?,
        url: r.get(4)?,
    })
}

fn format_release(version: &str, rows: &[ReleaseRow]) -> Value {
    let url = rows
        .iter()
        .find_map(|r| r.url.clone().filter(|u| !u.is_empty()))
        .unwrap_or_default();
    let published = rows
        .iter()
        .find_map(|r| r.published.clone().filter(|p| !p.is_empty()))
        .unwrap_or_default();
    let joined: Vec<String> = rows
        .iter()
        .map(|r| {
            if r.section.is_empty() {
                r.body.clone()
            } else {
                format!("## {}\n\n{}", r.section, r.body)
            }
        })
        .collect();
    let mut body = joined.join("\n\n");
    let truncated = body.chars().count() > RELEASE_CAP;
    if truncated {
        body = truncate_chars(&body, RELEASE_CAP);
    }
    json!({
        "version": version,
        "published": published,
        "url": url,
        "truncated": truncated,
        "notes": body,
    })
}

// ------------------------------------------------------------ small helpers

/// Lowercase and strip everything outside [a-z0-9] (mirrors Python's re.sub).
fn norm_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        s.to_string()
    } else {
        s.chars().take(cap).collect()
    }
}

/// Numeric version comparison on dot-separated integer components.
fn parse_version_key(v: &str) -> Vec<u64> {
    v.split('.')
        .filter_map(|p| p.parse::<u64>().ok())
        .collect()
}

fn lt_versions(a: &[u64], b: &[u64]) -> bool {
    cmp_versions_slices(a, b) == Ordering::Less
}

fn le_versions(a: &[u64], b: &[u64]) -> bool {
    cmp_versions_slices(a, b) != Ordering::Greater
}

fn cmp_versions_slices(a: &[u64], b: &[u64]) -> Ordering {
    let n = a.len().max(b.len());
    for i in 0..n {
        let av = a.get(i).copied().unwrap_or(0);
        let bv = b.get(i).copied().unwrap_or(0);
        match av.cmp(&bv) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

fn cmp_versions(a: &str, b: &str) -> Ordering {
    cmp_versions_slices(&parse_version_key(a), &parse_version_key(b))
}

fn last_n(items: &[String], n: usize) -> Vec<String> {
    let start = items.len().saturating_sub(n);
    items[start..].to_vec()
}

// --------------------------------------------------------------- networking

fn fetch_releases_json(url: &str) -> Result<Value, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    let resp = agent
        .get(url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", "mcp-server-omarchy")
        .call();
    match resp {
        Ok(r) => r
            .into_string()
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------- MCP layer

fn tool_search_docs_schema() -> Value {
    json!({
        "name": "omarchy_search_docs",
        "description": "Search the offline Omarchy v4 (Quattro) knowledge base: the official manual, repo developer docs, shipped default configs, GitHub release notes, and full git commit history. Use for any 'how do I X in Omarchy' question before guessing. Results are grouped by trust tier (manual > configs > releases > commits). Zero/thin results include an escalation ladder for finding the answer elsewhere.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Natural language question or keywords"},
                "source_filter": {
                    "type": "string",
                    "enum": ["omarchy", "manual", "config", "release"],
                    "description": "Restrict to a corpus (default: all)"
                },
                "omarchy_version": {
                    "type": "string",
                    "description": "Only include release notes of this version, e.g. 4.0.0"
                },
                "limit": {"type": "integer", "minimum": 1, "maximum": 20, "description": "Max results (default 8)"}
            },
            "required": ["query"]
        }
    })
}

fn tools_list() -> Vec<Value> {
    vec![
        tool_search_docs_schema(),
        json!({
            "name": "omarchy_get_page",
            "description": "Fetch a complete Omarchy manual chapter or developer doc as markdown. Pass a title (e.g. 'Hotkeys'), a live omarchy.org/manual/... URL, or a repo path (e.g. manual/06-themes.md). Prefer calling after search.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name_or_url": {"type": "string"}
                },
                "required": ["name_or_url"]
            }
        }),
        json!({
            "name": "omarchy_whats_new",
            "description": "Release notes/changelog. No args = latest release (v4.0.0 Quattro). version='3.7.0' for a specific release; since='2026-01-01' for every release after a date.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "version": {"type": "string"},
                    "since": {"type": "string", "description": "ISO date, e.g. 2026-06-01"}
                }
            }
        }),
        json!({
            "name": "omarchy_find_config",
            "description": "Find where a setting lives: returns shipped default config file paths (hypr/*.lua, foot.ini, /etc overrides…), content matches, and manual references that mention config locations.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "app": {"type": "string", "description": "App or feature, e.g. waybar, hyprland, gtk"}
                },
                "required": ["app"]
            }
        }),
        json!({
            "name": "omarchy_diff_releases",
            "description": "What changed between two Omarchy versions, e.g. from 3.8.4 to 4.0.0 — the first stop for 'I broke my system with an update'. Use breaking_only=true to filter to releases flagging breaking changes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_version": {"type": "string", "description": "e.g. 3.8.4"},
                    "to_version": {"type": "string", "description": "e.g. 4.0.0"},
                    "breaking_only": {"type": "boolean"}
                },
                "required": ["from_version", "to_version"]
            }
        }),
        json!({
            "name": "omarchy_check_updates",
            "description": "Check whether this KB snapshot is behind upstream Omarchy (one GitHub API call, only when invoked). Returns OUTDATED + the refresh command when a newer release exists. Call before debugging version-sensitive behavior or when answers seem to miss recent features.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "omarchy_kb_info",
            "description": "Knowledge base stats: sources, chunk counts, pinned versions.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
    ]
}

/// Slim a search hit down to the wire representation.
fn slim(rec: &Rec) -> Value {
    let mut out = Map::new();
    out.insert("source".into(), json!(rec.source));
    out.insert("title".into(), json!(rec.title));
    out.insert("section".into(), json!(rec.section));
    out.insert("url".into(), json!(rec.url));
    out.insert("match".into(), json!(rec.exactness));
    out.insert("snippet".into(), json!(rec.snippet));
    let ellipsized = rec.body.chars().count() > BODY_CAP;
    let mut body = truncate_chars(&rec.body, BODY_CAP);
    if ellipsized {
        body.push('\u{2026}');
    }
    out.insert("body".into(), json!(body));
    if let Some(v) = &rec.version {
        if !v.is_empty() {
            out.insert("version".into(), json!(v));
        }
    }
    if let Some(p) = &rec.published {
        if !p.is_empty() {
            out.insert("published".into(), json!(p));
        }
    }
    if !rec.path.is_empty() {
        out.insert("path".into(), json!(rec.path));
    }
    Value::Object(out)
}

fn make_result(payload: &Value) -> Value {
    json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(payload).unwrap_or_default()}]
    })
}

fn handle_call(kb: &KnowledgeBase, name: &str, args: &Value) -> Result<Value, String> {
    let get = |k: &str| args.get(k).and_then(Value::as_str);
    match name {
        "omarchy_search_docs" => {
            let query = get("query")
                .ok_or_else(|| "'query'".to_string())?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|l| l as usize)
                .unwrap_or(8)
                .clamp(1, 1000);
            let res = kb.search(
                query,
                get("source_filter"),
                get("omarchy_version"),
                limit,
            );
            let results: Vec<Value> = res.iter().map(slim).collect();
            let mut payload = Map::new();
            payload.insert("query".into(), json!(query));
            payload.insert("results".into(), Value::Array(results.clone()));
            if results.len() < 2 {
                payload.insert(
                    "did_you_mean".into(),
                    json!(suggest_terms(&kb.con, &tokenize(query))),
                );
                payload.insert(
                    "escalation".into(),
                    json!({
                        "retry_hints": ESCALATION_RETRY_HINTS,
                        "next_steps": ESCALATION_NEXT_STEPS,
                    }),
                );
            }
            Ok(Value::Object(payload))
        }
        "omarchy_get_page" => {
            let q = get("name_or_url")
                .ok_or_else(|| "'name_or_url'".to_string())?;
            Ok(kb.get_page(q))
        }
        "omarchy_whats_new" => Ok(kb.whats_new(get("version"), get("since"))),
        "omarchy_find_config" => {
            let app = get("app").ok_or_else(|| "'app'".to_string())?;
            Ok(kb.find_config(app))
        }
        "omarchy_diff_releases" => {
            let from = get("from_version")
                .ok_or_else(|| "'from_version'".to_string())?;
            let to = get("to_version")
                .ok_or_else(|| "'to_version'".to_string())?;
            let breaking = args
                .get("breaking_only")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(kb.diff_releases(from, to, breaking))
        }
        "omarchy_check_updates" => Ok(kb.check_updates()),
        "omarchy_kb_info" => Ok(kb.kb_info()),
        _ => Err(format!("Unknown tool {name}")),
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": code, "message": message}
    })
}

fn emit(line: &str) {
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = writeln!(lock, "{line}");
    let _ = lock.flush();
}

use std::cmp::Ordering;

fn main() {
    // Fast path for the smoke/differential harnesses: dump tool metadata and exit.
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "--tools-json") {
        println!("{}", serde_json::to_string(&tools_list()).unwrap());
        return;
    }
    if argv.iter().any(|a| a == "--version") {
        println!("{SERVER_NAME} {SERVER_VERSION}");
        return;
    }

    let root = match project_root() {
        Some(r) => r,
        None => {
            log("could not locate data/kb.sqlite3; set OMARCHY_KB_ROOT or pass --kb");
            std::process::exit(1);
        }
    };
    let db_path = root.join("data/kb.sqlite3");
    if !db_path.exists() {
        log(&format!(
            "Knowledge base missing at {}. Run scripts/build_kb.py first.",
            db_path.display()
        ));
        std::process::exit(1);
    }
    let kb = match KnowledgeBase::open(&db_path) {
        Ok(kb) => kb,
        Err(e) => {
            log(&format!("cannot open database {}: {e}", db_path.display()));
            std::process::exit(1);
        }
    };
    log(&format!(
        "ready: {} chunks from {}",
        kb.total_chunks(),
        db_path.display()
    ));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                emit(
                    &serde_json::to_string(&rpc_error(
                        Value::Null,
                        -32700,
                        &format!("Parse error: {e}"),
                    ))
                    .unwrap(),
                );
                continue;
            }
        };

        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let id = req.get("id").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => {
                let proto = req
                    .pointer("/params/protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05");
                emit(
                    &json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "protocolVersion": proto,
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION},
                        }
                    })
                    .to_string(),
                );
            }
            "notifications/initialized" => {}
            "ping" => emit(&json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string()),
            "tools/list" => emit(
                &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools_list()}})
                    .to_string(),
            ),
            "tools/call" => {
                let name = req.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
                let arguments = req.pointer("/params/arguments").cloned().unwrap_or(json!({}));
                match handle_call(&kb, name, &arguments) {
                    Ok(payload) => emit(
                        &json!({"jsonrpc": "2.0", "id": id, "result": make_result(&payload)})
                            .to_string(),
                    ),
                    Err(e) => {
                        log(&format!("tool error: {e}"));
                        emit(
                            &json!({
                                "jsonrpc": "2.0", "id": id,
                                "result": {
                                    "content": [{"type": "text", "text": format!("Error: {e}")}],
                                    "isError": true
                                }
                            })
                            .to_string(),
                        );
                    }
                }
            }
            _ => {
                // Requests carry an id; notifications (absent/null id) get no reply.
                let wants_reply = matches!(req.get("id"), Some(v) if !v.is_null());
                if wants_reply {
                    emit(
                        &serde_json::to_string(&rpc_error(
                            id,
                            -32601,
                            &format!("Method not found: {method}"),
                        ))
                        .unwrap(),
                    );
                }
            }
        }
    }
}
