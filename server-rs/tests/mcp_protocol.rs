//! End-to-end MCP protocol tests: speak newline-delimited JSON-RPC over the
//! server binary's stdio, the same way an MCP host does.

use std::io::{BufRead, BufWriter, Write};
use std::process::{Child, Command, Stdio};

use serde_json::{json, Value};

struct Client {
    child: Child,
    reader: std::io::Lines<std::io::BufReader<std::process::ChildStdout>>,
    writer: BufWriter<std::process::ChildStdin>,
    next_id: u64,
}

impl Client {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-server-omarchy-docs"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn server");
        let reader = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let writer = BufWriter::new(child.stdin.take().unwrap());
        Self {
            child,
            reader,
            writer,
            next_id: 1,
        }
    }

    fn send(&mut self, payload: Value) -> Option<Value> {
        let is_notification = payload.get("id").is_none();
        self.writer
            .write_all(format!("{payload}\n").as_bytes())
            .expect("write request");
        self.writer.flush().expect("flush request");
        if is_notification {
            return None;
        }
        loop {
            let line = self
                .reader
                .next()
                .expect("server closed stdout")
                .expect("read line");
            let resp: Value = serde_json::from_str(&line).expect("valid json response");
            if resp.get("id") == payload.get("id") {
                return Some(resp);
            }
        }
    }

    fn initialize(&mut self) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let resp = self
            .send(json!({
                "jsonrpc": "2.0", "id": id, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "test", "version": "0"}}
            }))
            .expect("initialize response");
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        resp["result"].clone()
    }

    fn call_tool(&mut self, name: &str, args: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let resp = self
            .send(json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": {"name": name, "arguments": args}
            }))
            .expect("tools/call response");
        assert!(resp.get("error").is_none(), "RPC error: {resp}");
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap())
            .expect("tool payload is JSON")
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn client() -> Client {
    let mut c = Client::spawn();
    let result = c.initialize();
    assert_eq!(result["serverInfo"]["name"], "omarchy-docs");
    c
}

#[test]
fn ping_roundtrip() {
    let mut c = Client::spawn();
    c.initialize();
    let id = c.next_id;
    c.next_id += 1;
    let resp = c
        .send(json!({"jsonrpc": "2.0", "id": id, "method": "ping"}))
        .unwrap();
    assert_eq!(resp["result"], json!({}));
}

#[test]
fn tools_list_has_all_seven_tools() {
    let mut c = Client::spawn();
    c.initialize();
    let id = c.next_id;
    c.next_id += 1;
    let resp = c
        .send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/list"}))
        .unwrap();
    let mut names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "omarchy_check_updates",
            "omarchy_diff_releases",
            "omarchy_find_config",
            "omarchy_get_page",
            "omarchy_kb_info",
            "omarchy_search_docs",
            "omarchy_whats_new",
        ]
    );
}

#[test]
fn search_finds_theme_docs_in_manual_tier() {
    let mut c = client();
    let r = c.call_tool(
        "omarchy_search_docs",
        json!({"query": "how do I switch themes?"}),
    );
    let results = r["results"].as_array().unwrap();
    assert!(
        results.iter().any(|x| x["source"] == "manual"
            && x["title"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("theme")),
        "expected a manual theme hit, got: {results:?}"
    );
}

#[test]
fn plugin_dev_guide_is_searchable() {
    let mut c = client();
    let r = c.call_tool(
        "omarchy_search_docs",
        json!({"query": "omarchy plugin clone clock BarWidget", "source_filter": "plugin-dev"}),
    );
    let results = r["results"].as_array().unwrap();
    assert!(
        results.iter().any(|x| x["source"] == "plugin-dev"
            && x["url"]
                .as_str()
                .unwrap_or("")
                .contains("omarchyplugins.com/develop")),
        "expected plugin-dev develop.html hit, got: {results:?}"
    );
}

#[test]
fn get_page_loads_plugin_develop_guide() {
    let mut c = client();
    let r = c.call_tool(
        "omarchy_get_page",
        json!({"name_or_url": "https://omarchyplugins.com/develop.html"}),
    );
    assert_eq!(r["source"], "plugin-dev");
    let md = r["markdown"].as_str().unwrap_or("");
    assert!(
        md.contains("BarWidget"),
        "missing BarWidget example: {}",
        &md[..md.len().min(200)]
    );
    assert!(
        md.contains("omarchy plugin clone") || md.contains("omarchy plugin validate"),
        "missing clone/validate commands"
    );
}

#[test]
fn manual_filter_ranks_themes_chapter() {
    let mut c = client();
    let r = c.call_tool(
        "omarchy_search_docs",
        json!({"query": "themes", "source_filter": "manual"}),
    );
    let results = r["results"].as_array().unwrap();
    assert!(
        results
            .iter()
            .take(4)
            .any(|x| x["title"] == "Themes" && x["section"] == "Themes"),
        "Themes chapter not ranked: {results:?}"
    );
}

#[test]
fn zero_result_search_includes_did_you_mean_and_escalation() {
    let mut c = client();
    let r = c.call_tool("omarchy_search_docs", json!({"query": "zzzqqqxyzzy"}));
    assert_eq!(r["results"].as_array().unwrap().len(), 0);
    assert!(r.get("did_you_mean").is_some());
    assert!(r.get("escalation").is_some());
}

#[test]
fn get_page_by_title_url_and_errors() {
    let mut c = client();

    let r = c.call_tool("omarchy_get_page", json!({"name_or_url": "Hotkeys"}));
    assert!(r["chars"].as_u64().unwrap() > 3000);

    let r = c.call_tool(
        "omarchy_get_page",
        json!({"name_or_url": "https://omarchy.org/manual/dual-boot-install/"}),
    );
    assert!(r["title"].as_str().unwrap().contains("Dual Boot"));

    let r = c.call_tool(
        "omarchy_get_page",
        json!({"name_or_url": "zzz-no-such-page"}),
    );
    assert_eq!(
        r["error"], "No page titled 'zzz-no-such-page'",
        "unexpected error payload: {r}"
    );
}

#[test]
fn whats_new_latest_since_and_unknown_version() {
    let mut c = client();

    let info = c.call_tool("omarchy_kb_info", json!({}));
    let pinned = info["latest_release"]
        .as_str()
        .unwrap()
        .trim_start_matches('v');
    let r = c.call_tool("omarchy_whats_new", json!({}));
    assert_eq!(r["version"], pinned);
    assert!(r["notes"].as_str().unwrap().len() > 100);

    let r = c.call_tool("omarchy_whats_new", json!({"since": "2026-08-01"}));
    let releases = r["releases"].as_array().unwrap();
    assert!(!releases.is_empty());
    assert!(releases
        .iter()
        .all(|rel| rel["published"].as_str().unwrap() > "2026-08-01"));

    let r = c.call_tool("omarchy_whats_new", json!({"version": "9.9.9"}));
    assert_eq!(r["error"], "No release '9.9.9'");
    assert!(r.get("available").is_some());
}

#[test]
fn diff_releases_range_breaking_and_unknown() {
    let mut c = client();

    let r = c.call_tool(
        "omarchy_diff_releases",
        json!({"from_version": "3.8.4", "to_version": "4.0.0"}),
    );
    assert_eq!(r["count"].as_u64().unwrap(), 1);
    assert_eq!(r["releases"][0]["version"], "4.0.0");

    let r = c.call_tool(
        "omarchy_diff_releases",
        json!({"from_version": "4.0.0", "to_version": "3.8.4"}),
    );
    assert!(r.get("error").is_some());

    let r = c.call_tool(
        "omarchy_diff_releases",
        json!({"from_version": "1.2.3", "to_version": "4.0.0"}),
    );
    assert_eq!(r["error"], "Unknown version '1.2.3'");
}

#[test]
fn find_config_hits_hyprland_defaults() {
    let mut c = client();
    let r = c.call_tool("omarchy_find_config", json!({"app": "hyprland"}));
    let paths: Vec<&str> = r["config_files_by_path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("hypr/hyprland.lua")),
        "expected hypr/hyprland.lua among {paths:?}"
    );
}

#[test]
fn kb_info_reports_manual_counts() {
    let mut c = client();
    let r = c.call_tool("omarchy_kb_info", json!({}));
    assert!(r["chunks"]["manual"].as_u64().unwrap() > 150);
    assert_eq!(r["server"], "omarchy-docs");
}
