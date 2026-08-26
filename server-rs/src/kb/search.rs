//! Tiered full-text search: AND/OR FTS expressions, per-tier caps, upstream
//! see-also links, and version filtering.

use std::collections::HashSet;

use super::lexicon::{expand_synonyms, fts_escape, tokenize};
use super::{query, tier_limit, KnowledgeBase, Rec, TIERS};

/// One ranked FTS pass over the given sources.
fn run_match(kb: &KnowledgeBase, match_expr: &str, sources: &[&str], limit: usize) -> Vec<Rec> {
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
    let mut params = vec![rusqlite::types::Value::from(match_expr.to_string())];
    params.extend(sources.iter().map(|s| rusqlite::types::Value::from(s.to_string())));
    query(&kb.con, &sql, &params, Rec::from_row)
}

/// Dedup key for a hit: same source+title+section is the same chunk.
fn dedup_key(rec: &Rec) -> (String, String, String) {
    (rec.source.clone(), rec.title.clone(), rec.section.clone())
}

/// A tier table: ordered (name, sources) pairs. Element strings are always
/// static literals; the table itself may be built per-call.
type TierTable = [(&'static str, &'static [&'static str])];

/// Best-of-each-tier in priority order (manual > configs > releases > commits).
/// Like the Python original, each pass starts from a fresh seen-set.
fn collect_tiered(
    kb: &KnowledgeBase,
    expr: &str,
    tiers: &TierTable,
    exactness: &'static str,
    limit: usize,
) -> Vec<Rec> {
    let mut results: Vec<Rec> = Vec::new();
    let mut seen: HashSet<(String, String, String)> = HashSet::new();
    for &(tier_name, sources) in tiers {
        let cap = tier_limit(tier_name);
        let mut taken = 0usize;
        for row in run_match(kb, expr, sources, cap * 2 + limit) {
            let key = dedup_key(&row);
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
}

/// AND over all tokens first; fall back to OR (tokens + synonyms) when thin.
/// The OR pass is deduplicated against the AND pass only afterwards.
fn build_and_or_expressions(
    kb: &KnowledgeBase,
    tokens: &[String],
) -> (String, String) {
    let synonyms = expand_synonyms(tokens, &kb.synonyms);
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
    (and_expr, or_expr)
}

impl KnowledgeBase {
    pub(crate) fn search(
        &self,
        query_text: &str,
        source_filter: Option<&str>,
        omarchy_version: Option<&str>,
        limit: usize,
    ) -> Vec<Rec> {
        let tokens = tokenize(query_text);
        if tokens.is_empty() {
            return Vec::new();
        }

        let tiers: &TierTable = match source_filter {
            Some("omarchy") => super::OMARCHY_TIERS,
            Some("plugin-dev") => super::PLUGIN_DEV_TIERS,
            Some("manual") | Some("docs") => &TIERS[0..1],
            Some("config") => &TIERS[1..2],
            Some("release") => &TIERS[2..3],
            _ => TIERS,
        };

        let (and_expr, or_expr) = build_and_or_expressions(self, &tokens);
        let mut out: Vec<Rec> = collect_tiered(self, &and_expr, tiers, "exact", limit);
        if out.len() < limit {
            let mut seen_keys: HashSet<(String, String, String)> =
                out.iter().map(dedup_key).collect();
            for rec in collect_tiered(self, &or_expr, tiers, "partial", limit) {
                let key = dedup_key(&rec);
                if !seen_keys.contains(&key) {
                    seen_keys.insert(key);
                    out.push(rec);
                }
            }
        }

        attach_upstream_links(self, query_text, &mut out);
        out.truncate(limit);
        filter_by_version(out, omarchy_version)
    }
}

/// Tag results with curated upstream links when the query matches.
fn attach_upstream_links(kb: &KnowledgeBase, query: &str, recs: &mut [Rec]) {
    let q_lower = query.to_lowercase();
    let see_also: Vec<String> = kb
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
        for rec in recs.iter_mut() {
            rec.see_also = see_also.clone();
        }
    }
}

/// Keep release hits only for the requested version.
fn filter_by_version(recs: Vec<Rec>, omarchy_version: Option<&str>) -> Vec<Rec> {
    let Some(wanted) = omarchy_version else {
        return recs;
    };
    let wanted = wanted.trim_start_matches('v');
    recs.into_iter()
        .filter(|r| {
            r.source != "release"
                || r.version
                    .as_deref()
                    .map(|rv| rv.trim_start_matches('v') == wanted)
                    .unwrap_or(false)
        })
        .collect()
}

