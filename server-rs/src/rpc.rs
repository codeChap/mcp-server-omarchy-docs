//! MCP wire layer: newline-delimited JSON-RPC over stdio, tool dispatch,
//! and response shaping.

use std::io::{BufRead, Write};

use serde_json::{json, Map, Value};

use crate::kb::{suggest_terms, tokenize, KnowledgeBase};
use crate::tools::tools_list;
use crate::util::truncate_chars;
use crate::{log, SERVER_NAME, SERVER_VERSION};

const BODY_CAP: usize = 1600;

const ESCALATION_RETRY_HINTS: &[&str] = &[
    "Use Omarchy's exact vocabulary: quickshell, keybind, theme, snapshot, factory reset",
    "Try one distinctive noun instead of a full sentence",
    "Config/keybind/default question? Use omarchy_find_config",
    "'When did X change?' questions belong in omarchy_whats_new",
];

const ESCALATION_NEXT_STEPS: &str = "Thin results may mean the topic is undocumented. Escalate: \
     (1) read the source at github.com/omacom/omarchy — bin/ scripts and \
     git log are the real docs for internals; \
     (2) upstream docs — quickshell.org/docs (v4 shell), wiki.hypr.land \
     (window manager options), wiki.archlinux.org (everything below); \
     (3) search closed issues at github.com/omacom/omarchy/issues; \
     (4) ask on the Omarchy Discord discord.gg/tXFUdasqhY with omarchy-debug output attached. \
     Never invent Omarchy-specific behavior from general Arch/Hyprland knowledge; \
     if unverified, say so.";

/// Slim a search hit down to the wire representation.
fn slim(rec: &crate::kb::Rec) -> Value {
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
            let query = get("query").ok_or_else(|| "'query'".to_string())?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .map(|l| l as usize)
                .unwrap_or(8)
                .clamp(1, 1000);
            let res = kb.search(query, get("source_filter"), get("omarchy_version"), limit);
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
            let q = get("name_or_url").ok_or_else(|| "'name_or_url'".to_string())?;
            Ok(kb.get_page(q))
        }
        "omarchy_whats_new" => Ok(kb.whats_new(get("version"), get("since"))),
        "omarchy_find_config" => {
            let app = get("app").ok_or_else(|| "'app'".to_string())?;
            Ok(kb.find_config(app))
        }
        "omarchy_diff_releases" => {
            let from = get("from_version").ok_or_else(|| "'from_version'".to_string())?;
            let to = get("to_version").ok_or_else(|| "'to_version'".to_string())?;
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

/// Serve requests until stdin closes. Returns only on fatal setup errors.
pub(crate) fn serve(kb: &KnowledgeBase) {
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
            "initialize" => initialize_reply(&req, id),
            "notifications/initialized" => {}
            "ping" => emit(&json!({"jsonrpc": "2.0", "id": id, "result": {}}).to_string()),
            "tools/list" => emit(
                &json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools_list()}}).to_string(),
            ),
            "tools/call" => tools_call(kb, &req, id),
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

fn initialize_reply(req: &Value, id: Value) {
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

fn tools_call(kb: &KnowledgeBase, req: &Value, id: Value) {
    let name = req
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let arguments = req
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(json!({}));
    match handle_call(kb, name, &arguments) {
        Ok(payload) => {
            emit(&json!({"jsonrpc": "2.0", "id": id, "result": make_result(&payload)}).to_string())
        }
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
