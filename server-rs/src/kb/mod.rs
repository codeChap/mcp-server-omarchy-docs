//! The SQLite-backed Omarchy knowledge base and its tool-facing queries.
//!
//! Split by concern across sibling files; all of them add methods to
//! `KnowledgeBase` so call sites keep using method syntax. Every query
//! swallows errors and returns an empty result set — the behavior throughout.

mod find_config;
mod lexicon;
mod pages;
mod releases;
mod search;
mod updates;

pub(crate) use lexicon::{suggest_terms, tokenize};

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params_from_iter, Connection};
use serde_json::{Map, Value};

/// Result tiers: the current manual beats repo internals beats shipped configs
/// beats historical release notes, regardless of raw bm25 score.
pub(crate) const TIERS: &[(&str, &[&str])] = &[
    ("core", &["manual", "repo-docs"]),
    ("configs", &["config"]),
    ("releases", &["release"]),
    ("history", &["commit"]),
];

/// Reduced tier table for source_filter = "omarchy" (docs, then releases).
pub(crate) const OMARCHY_TIERS: &[(&str, &[&str])] =
    &[("core", &["manual", "repo-docs"]), ("releases", &["release"])];

pub(crate) fn tier_limit(tier: &str) -> usize {
    match tier {
        "core" => 6,
        "configs" => 3,
        "releases" => 4,
        _ => 3,
    }
}

/// One search hit before response slimming. Mirrors the Python dict shape.
#[derive(Clone)]
pub(crate) struct Rec {
    pub(crate) source: String,
    pub(crate) title: String,
    pub(crate) section: String,
    pub(crate) body: String,
    pub(crate) url: String,
    pub(crate) path: String,
    pub(crate) version: Option<String>,
    pub(crate) published: Option<String>,
    pub(crate) snippet: String,
    pub(crate) tier: &'static str,
    pub(crate) exactness: &'static str,
    pub(crate) see_also: Vec<String>,
}

impl Rec {
    fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
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
    }
}

#[derive(Clone)]
pub(crate) struct ReleaseRow {
    pub(crate) version: String,
    #[allow(dead_code)]
    pub(crate) published: Option<String>,
    pub(crate) section: String,
    pub(crate) body: String,
    #[allow(dead_code)]
    pub(crate) url: Option<String>,
}

pub(crate) fn map_release_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ReleaseRow> {
    Ok(ReleaseRow {
        version: r.get::<_, Option<String>>(0)?.unwrap_or_default(),
        published: r.get(1)?,
        section: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        body: r.get(3)?,
        url: r.get(4)?,
    })
}

pub(crate) struct KnowledgeBase {
    pub(crate) con: Connection,
    pub(crate) synonyms: Map<String, Value>,
    pub(crate) upstream_map: Map<String, Value>,
}

/// Run one mapped query against the KB connection; any prepare or row error
/// yields an empty Vec, exactly like every original inline block.
pub(crate) fn query<T>(
    con: &Connection,
    sql: &str,
    params: &[rusqlite::types::Value],
    map: impl Fn(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> Vec<T> {
    let mut stmt = match con.prepare(sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map(params_from_iter(params.iter().cloned()), map)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

impl KnowledgeBase {
    pub(crate) fn open(db_path: &Path) -> rusqlite::Result<Self> {
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
    pub(crate) fn meta_map(&self) -> HashMap<String, String> {
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

    pub(crate) fn counts(&self) -> Vec<(String, i64)> {
        query(&self.con, "SELECT source, COUNT(*) FROM chunks GROUP BY source", &[], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
    }

    pub(crate) fn total_chunks(&self) -> i64 {
        self.counts().iter().map(|(_, n)| n).sum()
    }
}

/// Load a JSON object from disk; any error yields an empty map.
fn load_json(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
