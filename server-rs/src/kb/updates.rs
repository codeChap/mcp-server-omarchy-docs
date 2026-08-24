//! Snapshot freshness: kb_info, snapshot_info, and the networked
//! check_updates against the upstream GitHub releases API.

use std::cmp::Ordering;

use serde_json::{json, Map, Value};

use super::KnowledgeBase;
use crate::util::cmp_versions;
use crate::{SERVER_NAME, SERVER_VERSION};

const RELEASES_API: &str =
    "https://api.github.com/repos/basecamp/omarchy/releases?per_page=5";

impl KnowledgeBase {
    pub(crate) fn check_updates(&self) -> Value {
        // Compare the KB snapshot against upstream. Network call on invoke.
        let m = self.meta_map();
        let local_latest = m
            .get("latest_release")
            .map(|s| s.trim_start_matches('v').to_string())
            .unwrap_or_default();

        let fetch = fetch_releases_json(RELEASES_API);
        let info = self.snapshot_info();
        match fetch {
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
            Ok(releases) => self.merge_upstream_status(info, &releases, &local_latest),
        }
    }

    fn merge_upstream_status(&self, mut info: Value, releases: &Value, local_latest: &str) -> Value {
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
        let differs =
            matches!(&newest, Some(n) if *n != local_latest) && !local_latest.is_empty();
        if !differs {
            let obj = match info.as_object_mut() {
                Some(o) => o,
                None => return json!({}),
            };
            obj.insert(
                "upstream_newest_release".into(),
                newest.map(Value::String).unwrap_or(Value::Null),
            );
            obj.insert("status".into(), json!("UP TO DATE"));
            return info;
        }
        let newest_str = newest.as_deref().unwrap_or_default();
        let outdated = cmp_versions(newest_str, local_latest) == Ordering::Greater;
        let status = if outdated {
            "OUTDATED — refresh now".to_string()
        } else {
            "KB is ahead of tagged releases (unreleased commits indexed)".to_string()
        };
        if let Some(obj) = info.as_object_mut() {
            obj.insert("upstream_newest_release".into(), json!(newest_str));
            obj.insert("status".into(), Value::String(status));
            obj.insert(
                "refresh".into(),
                json!("scripts/fetch_sources.sh && (cd server-rs && cargo build --release && ./target/release/build-kb)"),
            );
            obj.insert(
                "recent_upstream_releases".into(),
                json!(upstream.iter().take(5).collect::<Vec<_>>()),
            );
        }
        info
    }

    pub(crate) fn snapshot_info(&self) -> Value {
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

    pub(crate) fn kb_info(&self) -> Value {
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
            "snapshot_note": "Point-in-time snapshot. Run omarchy_check_updates (network) to compare against upstream, or just refresh: scripts/fetch_sources.sh && (cd server-rs && cargo build --release && ./target/release/build-kb)",
            "corpus": "omarchy.org Quattro manual + basecamp/omarchy repo docs, defaults, releases & git history",
        })
    }
}

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
