//! Release notes: whats_new lookups, version-range diffs, and release
//! formatting.

use rusqlite::types::Value as SqlValue;
use serde_json::{json, Value};

use super::map_release_row;
use super::{query, KnowledgeBase, ReleaseRow};
use crate::util::{
    cmp_versions, last_n, le_versions, lt_versions, parse_version_key, truncate_chars,
};

const RELEASE_CAP: usize = 40_000;

impl KnowledgeBase {
    pub(crate) fn release_rows_for_version(&self, version: &str) -> Vec<ReleaseRow> {
        query(
            &self.con,
            "SELECT version, published, section, body, url FROM chunks \
             WHERE source='release' AND version=?1 ORDER BY id",
            &[SqlValue::from(version.to_string())],
            map_release_row,
        )
    }

    pub(crate) fn all_release_rows(&self) -> Vec<ReleaseRow> {
        query(
            &self.con,
            "SELECT version, published, section, body, url FROM chunks \
             WHERE source='release' ORDER BY published DESC, id",
            &[],
            map_release_row,
        )
    }

    pub(crate) fn whats_new(&self, version: Option<&str>, since: Option<&str>) -> Value {
        if let Some(version) = version {
            return self.whats_new_for_version(version);
        }

        if let Some(since) = since {
            let versions = self.release_versions_since(since);
            if versions.is_empty() {
                return json!({ "error": format!("No releases after {since}") });
            }
            let all_rows = self.all_release_rows();
            let mut releases = Vec::new();
            for v in versions {
                let vr: Vec<ReleaseRow> = all_rows
                    .iter()
                    .filter(|r| r.version == v)
                    .cloned()
                    .collect();
                releases.push(format_release(&v, &vr));
            }
            return json!({ "count": releases.len(), "releases": releases });
        }

        // Latest release overall.
        match self.latest_release_version() {
            Some(v) if !v.is_empty() => {
                let rows = self.release_rows_for_version(&v);
                format_release(&v, &rows)
            }
            _ => json!({ "error": "No release notes indexed" }),
        }
    }

    fn whats_new_for_version(&self, version: &str) -> Value {
        let v = version.trim_start_matches('v');
        let no_dots = v.replace('.', "");
        // LIKE '%40%' would also match 4.10.x etc.; filter precisely after.
        let pattern = format!("%{no_dots}%");
        let mut rows: Vec<ReleaseRow> = query(
            &self.con,
            "SELECT version, published, section, body, url FROM chunks \
             WHERE source='release' AND REPLACE(version,'.','') LIKE ?1 \
             ORDER BY published DESC, id",
            &[SqlValue::from(pattern)],
            map_release_row,
        );
        rows.retain(|r| r.version.trim_start_matches('v') == v);
        if rows.is_empty() {
            return json!({
                "error": format!("No release '{}'", version),
                "available": self.recent_versions(15),
            });
        }
        format_release(&rows[0].version.clone(), &rows)
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
        query(
            &self.con,
            "SELECT version, MIN(id) AS min_id FROM chunks \
             WHERE source='release' AND published > ?1 \
             GROUP BY version ORDER BY published DESC, min_id",
            &[SqlValue::from(since.to_string())],
            |r| r.get::<_, String>(0),
        )
    }

    pub(crate) fn diff_releases(
        &self,
        from_version: &str,
        to_version: &str,
        breaking_only: bool,
    ) -> Value {
        let known = self.known_versions_ascending();
        let strip = |v: &str| v.trim_start_matches('v').to_string();
        let f = strip(from_version);
        let t = strip(to_version);
        let has = |v: &str| known.iter().any(|k| k == v);
        for (given, bare) in [(&from_version, &f), (&to_version, &t)] {
            if !has(bare) && !has(&format!("v{bare}")) {
                return unknown_version_error(given, &known);
            }
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
            let vr: Vec<ReleaseRow> = all_rows
                .iter()
                .filter(|r| r.version == *v)
                .cloned()
                .collect();
            let rel = format_release(v, &vr);
            if breaking_only && !flags_breaking(&rel) {
                continue;
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

    pub(crate) fn known_versions_ascending(&self) -> Vec<String> {
        query(
            &self.con,
            "SELECT DISTINCT version FROM chunks WHERE source='release' ORDER BY published",
            &[],
            |r| r.get::<_, Option<String>>(0),
        )
        .into_iter()
        .flatten()
        .collect()
    }

    /// Newest known versions, descending.
    pub(crate) fn recent_versions(&self, n: usize) -> Vec<String> {
        let mut versions: Vec<String> = self
            .all_release_rows()
            .into_iter()
            .map(|r| r.version)
            .collect();
        versions.sort_by(|a, b| cmp_versions(b, a));
        versions.dedup();
        versions.into_iter().take(n).collect()
    }
}

fn unknown_version_error(given: &str, known: &[String]) -> Value {
    json!({
        "error": format!("Unknown version '{}'", given),
        "known": last_n(known, 10),
    })
}

/// Keyword heuristic mirroring the Python port's breaking-change flagging.
fn flags_breaking(release: &Value) -> bool {
    const MARKERS: &[&str] = &[
        "breaking",
        "removed",
        "renamed",
        "migration",
        "moved to",
        "now requires",
        "incompatible",
    ];
    let text = release["notes"].as_str().unwrap_or("").to_lowercase();
    MARKERS.iter().any(|m| text.contains(m))
}

pub(crate) fn format_release(version: &str, rows: &[ReleaseRow]) -> Value {
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
