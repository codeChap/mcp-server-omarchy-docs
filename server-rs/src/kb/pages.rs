//! Page retrieval: by URL, repo path, or (fuzzy) title across manual,
//! repo-doc, and plugin-dev sources.

use rusqlite::types::Value as SqlValue;
use serde_json::{json, Value};

use super::query;
use super::KnowledgeBase;
use crate::util::norm_key;

fn no_page_error(name: &str) -> Value {
    json!({ "error": format!("No page titled '{}'", name.replace('\'', "''")) })
}

impl KnowledgeBase {
    pub(crate) fn get_page(&self, name_or_url: &str) -> Value {
        let q = name_or_url.trim();
        let norm_q = norm_key(q);

        if q.starts_with("http://") || q.starts_with("https://") {
            let title: Option<String> = query(
                &self.con,
                "SELECT DISTINCT title FROM chunks WHERE url = ?1 LIMIT 1",
                &[SqlValue::from(q.to_string())],
                |r| r.get(0),
            )
            .into_iter()
            .next();
            if let Some(title) = title {
                return self.page_by_title(&title);
            }
        }
        if q.contains('/') || q.ends_with(".md") {
            let titles = self.titles_matching_path(q);
            return match titles.len() {
                0 => json!({ "error": format!("No page matching '{}'", q.replace('\'', "''")) }),
                1 => self.page_by_title(&titles[0]),
                _ => json!({ "error": "ambiguous", "candidates": titles }),
            };
        }

        // By (partial) title across manual, repo docs, and plugin guides
        let mut matches: Vec<String> = self
            .all_page_titles()
            .into_iter()
            .filter(|t| {
                let norm_t = norm_key(t);
                !norm_q.is_empty()
                    && (norm_q == norm_t || norm_t.contains(&norm_q) || norm_t.starts_with(&norm_q))
            })
            .collect();
        match matches.len() {
            0 => {
                let mut e = no_page_error(name_or_url);
                e["hint"] = json!("Use omarchy_search_docs first, then pass result.title or .url");
                e
            }
            1 => self.page_by_title(&matches.remove(0)),
            _ => {
                matches.sort();
                json!({ "error": "ambiguous", "candidates": matches })
            }
        }
    }

    fn all_page_titles(&self) -> Vec<String> {
        query(
            &self.con,
            "SELECT DISTINCT title FROM chunks WHERE source IN ('manual','repo-docs','plugin-dev')",
            &[],
            |r| r.get::<_, String>(0),
        )
    }

    fn titles_matching_path(&self, q: &str) -> Vec<String> {
        let like = format!("%{q}%");
        query(
            &self.con,
            "SELECT DISTINCT title FROM chunks WHERE path LIKE ?1 ORDER BY title",
            &[SqlValue::from(like)],
            |r| r.get::<_, String>(0),
        )
    }

    fn page_by_title(&self, title: &str) -> Value {
        let rows: Vec<(String, String, String, String, String)> = query(
            &self.con,
            "SELECT source, section, body, url, path FROM chunks \
             WHERE title = ?1 AND source IN ('manual','repo-docs','plugin-dev') ORDER BY id",
            &[SqlValue::from(title.to_string())],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                ))
            },
        );
        if rows.is_empty() {
            // Unlike the title-search miss above, this message keeps raw
            // quotes; tests pin both spellings.
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
}
