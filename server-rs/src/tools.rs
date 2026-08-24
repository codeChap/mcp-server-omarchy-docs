//! Tool metadata advertised via `tools/list` (and `--tools-json`).

use serde_json::{json, Value};

fn search_docs_schema() -> Value {
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

pub(crate) fn tools_list() -> Vec<Value> {
    vec![
        search_docs_schema(),
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
