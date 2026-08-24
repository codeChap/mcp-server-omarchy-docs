//! omarchy_find_config: locate shipped default config files by path or
//! content, plus manual references that mention config locations.

use rusqlite::types::Value as SqlValue;
use serde_json::{json, Value};

use super::lexicon::fts_escape;
use super::query;
use super::KnowledgeBase;
use crate::util::truncate_chars;

impl KnowledgeBase {
    pub(crate) fn find_config(&self, app: &str) -> Value {
        let like = format!("%{app}%");
        let config_files_by_path: Vec<Value> = query(
            &self.con,
            "SELECT path, body FROM chunks WHERE source='config' \
               AND (path LIKE ?1 OR title LIKE ?1) \
             ORDER BY LENGTH(path) LIMIT 12",
            &[SqlValue::from(like.clone())],
            |r| {
                Ok(json!({
                    "path": r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    "excerpt": truncate_chars(&r.get::<_, String>(1)?, 600),
                }))
            },
        );

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

        let doc_references = self.config_doc_references(app);

        json!({
            "app": app,
            "config_files_by_path": config_files_by_path,
            "config_files_by_content": config_files_by_content,
            "doc_references": doc_references,
        })
    }

    /// Manual/repo-doc pages that reference real config locations.
    fn config_doc_references(&self, app: &str) -> Vec<Value> {
        let sql = format!(
            "SELECT c.title AS page_title, c.section, c.url, \
                    highlight(chunks_fts, 2, '<<', '>>') AS snippet \
             FROM chunks_fts f JOIN chunks c ON c.id=f.rowid \
             WHERE chunks_fts MATCH ?1 AND c.source IN ('manual','repo-docs') \
               AND (c.body LIKE '%~/.config%' OR c.body LIKE '%/etc/%') \
             ORDER BY rank LIMIT 6"
        );
        let match_expr = format!("{}*", fts_escape(app));
        query(&self.con, &sql, &[SqlValue::from(match_expr)], |r| {
            Ok(json!({
                "page": r.get::<_, String>(0)?,
                "section": r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                "url": r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                "snippet": r.get::<_, String>(3)?,
            }))
        })
    }
}
