//! Query lexing: tokenization, synonym expansion, FTS5 quoting, and
//! "did you mean" suggestions.

use rusqlite::Connection;
use serde_json::{Map, Value};

const STOPWORDS: &[&str] = &[
    "how", "do", "does", "i", "a", "an", "the", "to", "in", "on", "for", "of", "my",
    "me", "is", "are", "it", "its", "what", "whats", "when", "where", "which", "can",
    "with", "and", "or", "get", "got",
];

pub(crate) fn tokenize(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '+' || c == '#' || c == '.' || c == '-'))
        .filter(|t| !t.is_empty() && !STOPWORDS.contains(t))
        .map(str::to_owned)
        .collect()
}

/// Quote a token for FTS5 so operators/punctuation stay literal.
pub(crate) fn fts_escape(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

pub(crate) fn expand_synonyms(tokens: &[String], synonyms: &Map<String, Value>) -> Vec<String> {
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
pub(crate) fn suggest_terms(con: &Connection, tokens: &[String]) -> Vec<String> {
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
