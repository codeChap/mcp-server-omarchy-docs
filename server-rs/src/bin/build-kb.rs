//! build-kb — Build the SQLite knowledge base from raw Omarchy sources.
//!
//! Rust port of the former `scripts/build_kb.py`. Corpus:
//!   - manual:     manual/NN-*.md from the omacom/omarchy repo (Quattro v4)
//!   - repo-docs:  repo-internal developer docs (docs/*.md)
//!   - plugin-dev: omarchyplugins.com authoring guides (develop + publish)
//!   - config:     shipped default configs (config/**, default/**, etc/**)
//!   - release:    GitHub release notes (data/raw/releases.json)
//!   - commit:     one chunk per non-merge commit (design rationale)
//!
//! Output: data/kb.sqlite3 with an FTS5 table + metadata. Rerunning rebuilds it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rusqlite::Connection;
use serde_json::Value;

const SCHEMA: &str = r#"
CREATE TABLE chunks(
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    title TEXT NOT NULL,
    section TEXT DEFAULT '',
    body TEXT NOT NULL,
    path TEXT DEFAULT '',
    url TEXT DEFAULT '',
    version TEXT,
    published TEXT,
    priority INTEGER DEFAULT 1
);
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    title, section, body, content='chunks', content_rowid='id',
    tokenize='porter unicode61'
);
CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, title, section, body)
    VALUES (new.id, new.title, new.section, new.body);
END;
CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, title, section, body)
    VALUES ('delete', old.id, old.title, old.section, old.body);
END;
"#;

/// Manual chapter -> live URL mapping comes from the official site search index
/// (chapter title -> url), fetched by scripts/fetch_sources.sh.
const SEARCH_INDEX_FILE: &str = "data/raw/manual-search-index.json";
const PLUGIN_GUIDES_DIR: &str = "data/raw/omarchyplugins";
const PLUGIN_GUIDES: &[(&str, &str)] = &[
    ("develop.html", "https://omarchyplugins.com/develop.html"),
    ("publish.html", "https://omarchyplugins.com/publish.html"),
];

const CONFIG_EXTS: &[&str] = &[
    ".conf", ".ini", ".toml", ".kdl", ".css", ".rasi", ".json", ".sh", ".bash", ".zsh", ".lua",
];
const CONFIG_SKIP_DIRS: &[&str] = &[
    "applications",
    "autostart",
    "environment.d",
    "fontconfig",
    "systemd",
    "wallpapers",
    "backgrounds",
];
const CONFIG_NAME_ALLOWLIST: &[&str] = &[
    "hyprland.conf",
    "hyprlock.conf",
    "hypridle.conf",
    "makefile",
];

const CONFIG_MAX_BYTES: u64 = 200_000;
const BODY_CAP: usize = 50_000;

// ------------------------------------------------------------------ helpers

/// Lowercase and strip everything outside [a-z0-9].
fn norm_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

fn truncate_chars(s: &str, cap: usize) -> String {
    s.chars().take(cap).collect()
}

/// Strip image-only lines and collapse whitespace; keep code fences intact.
fn clean_markdown(text: &str) -> String {
    let kept: Vec<&str> = text.lines().filter(|line| !is_image_line(line)).collect();
    collapse_newlines(&kept.join("\n")).trim().to_string()
}

/// Matches ^!\[[^\]]*\]\([^)]*\)\s*$ on a single line.
fn is_image_line(line: &str) -> bool {
    let line = line.trim_end();
    let rest = match line.strip_prefix("![") {
        Some(r) => r,
        None => return false,
    };
    let alt_end = match rest.find(']') {
        Some(i) => i,
        None => return false,
    };
    let rest = &rest[alt_end + 1..];
    match rest.strip_prefix('(') {
        Some(r) => r.ends_with(')'),
        None => false,
    }
}

/// Extract `<main>` and turn marketplace HTML into markdown.
fn html_main_to_markdown(html: &str) -> String {
    let Some(main_open) = html.find("<main") else {
        return String::new();
    };
    let after_open = &html[main_open..];
    let Some(gt) = after_open.find('>') else {
        return String::new();
    };
    let inner = &after_open[gt + 1..];
    let inner = inner.split("</main>").next().unwrap_or(inner);
    let md = html_fragment_to_markdown(inner);
    collapse_newlines(md.trim()).to_string()
}

fn html_fragment_to_markdown(html: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let chars: Vec<char> = html.chars().collect();
    let n = chars.len();
    let mut list_depth = 0usize;
    while i < n {
        if chars[i] == '<' {
            let (tag, is_close, attrs, next) = parse_tag(&chars, i);
            i = next;
            let name = tag.to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            if !is_close && SKIP_TAGS.contains(&name.as_str()) {
                i = skip_until_close(&chars, i, &name);
                continue;
            }
            if !is_close
                && (name == "pre" || (name == "div" && class_contains(&attrs, "code-block")))
            {
                if name == "div" && class_contains(&attrs, "code-block") {
                    // Prefer the inner <pre>; skip the copy-button chrome.
                    continue;
                }
                let (body, after) = take_until_close(&chars, i, "pre");
                i = after;
                let code = strip_tags(&body);
                let lang = guess_fence_lang(&code);
                out.push_str("\n\n```");
                out.push_str(lang);
                out.push('\n');
                out.push_str(code.trim_end());
                out.push_str("\n```\n\n");
                continue;
            }
            match (name.as_str(), is_close) {
                ("h1", false) => {
                    let (body, after) = take_until_close(&chars, i, "h1");
                    i = after;
                    out.push_str("\n\n# ");
                    out.push_str(&inline_md(&body));
                    out.push_str("\n\n");
                }
                ("h2", false) => {
                    let (body, after) = take_until_close(&chars, i, "h2");
                    i = after;
                    out.push_str("\n\n## ");
                    out.push_str(&inline_md(&body));
                    out.push_str("\n\n");
                }
                ("h3", false) => {
                    let (body, after) = take_until_close(&chars, i, "h3");
                    i = after;
                    out.push_str("\n\n### ");
                    out.push_str(&inline_md(&body));
                    out.push_str("\n\n");
                }
                ("p", false) => {
                    let (body, after) = take_until_close(&chars, i, "p");
                    i = after;
                    let t = inline_md(&body);
                    if !t.trim().is_empty() {
                        out.push_str("\n\n");
                        out.push_str(&t);
                        out.push_str("\n\n");
                    }
                }
                ("br", false) | ("br/", false) => out.push('\n'),
                ("li", false) => {
                    let (body, after) = take_until_close(&chars, i, "li");
                    i = after;
                    let t = inline_md(&body);
                    if !t.trim().is_empty() {
                        out.push('\n');
                        for _ in 0..list_depth.saturating_sub(1) {
                            out.push_str("  ");
                        }
                        out.push_str("- ");
                        out.push_str(t.trim());
                        out.push('\n');
                    }
                }
                ("ul" | "ol", false) => {
                    list_depth += 1;
                    out.push('\n');
                }
                ("ul" | "ol", true) => {
                    list_depth = list_depth.saturating_sub(1);
                    out.push('\n');
                }
                ("table", false) => {
                    let (body, after) = take_until_close(&chars, i, "table");
                    i = after;
                    out.push('\n');
                    out.push_str(&table_to_markdown(&body));
                    out.push('\n');
                }
                ("blockquote", false) => {
                    let (body, after) = take_until_close(&chars, i, "blockquote");
                    i = after;
                    for line in inline_md(&body).lines() {
                        out.push_str("\n> ");
                        out.push_str(line.trim());
                    }
                    out.push_str("\n\n");
                }
                _ => {}
            }
        } else {
            // Bare text outside tags: keep if not whitespace-only runs we already collapse.
            let start = i;
            while i < n && chars[i] != '<' {
                i += 1;
            }
            let raw: String = chars[start..i].iter().collect();
            let decoded = decode_entities(&raw);
            if !decoded.trim().is_empty() {
                out.push_str(&decoded);
            }
        }
    }
    out
}

const SKIP_TAGS: &[&str] = &["script", "style", "svg", "button", "iframe", "noscript"];

fn parse_tag(chars: &[char], i: usize) -> (String, bool, String, usize) {
    if i >= chars.len() || chars[i] != '<' {
        return (String::new(), false, String::new(), i + 1);
    }
    let mut j = i + 1;
    let is_close = j < chars.len() && chars[j] == '/';
    if is_close {
        j += 1;
    }
    while j < chars.len() && chars[j].is_whitespace() {
        j += 1;
    }
    let name_start = j;
    while j < chars.len()
        && (chars[j].is_ascii_alphanumeric() || chars[j] == '/' || chars[j] == '-')
    {
        if chars[j] == '/' {
            break;
        }
        j += 1;
    }
    let name: String = chars[name_start..j].iter().collect();
    let attr_start = j;
    while j < chars.len() && chars[j] != '>' {
        j += 1;
    }
    let attrs: String = chars[attr_start..j].iter().collect();
    if j < chars.len() {
        j += 1;
    }
    (name, is_close, attrs, j)
}

fn class_contains(attrs: &str, needle: &str) -> bool {
    attrs.to_ascii_lowercase().contains(needle)
}

fn skip_until_close(chars: &[char], i: usize, name: &str) -> usize {
    let close = format!("</{name}");
    let hay: String = chars[i..].iter().collect();
    if let Some(pos) = hay.to_ascii_lowercase().find(&close) {
        let after = i + pos;
        // advance past '>'
        let mut k = after;
        while k < chars.len() && chars[k] != '>' {
            k += 1;
        }
        return (k + 1).min(chars.len());
    }
    chars.len()
}

fn take_until_close(chars: &[char], start: usize, name: &str) -> (String, usize) {
    let close = format!("</{name}");
    let hay: String = chars[start..].iter().collect();
    let hay_l = hay.to_ascii_lowercase();
    if let Some(pos) = hay_l.find(&close) {
        let inner = &hay[..pos];
        let mut k = start + pos;
        while k < chars.len() && chars[k] != '>' {
            k += 1;
        }
        (inner.to_string(), (k + 1).min(chars.len()))
    } else {
        (hay, chars.len())
    }
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_entities(&out)
}

fn inline_md(html: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let (tag, is_close, attrs, next) = parse_tag(&chars, i);
            i = next;
            let name = tag.to_ascii_lowercase();
            if !is_close && SKIP_TAGS.contains(&name.as_str()) {
                i = skip_until_close(&chars, i, &name);
                continue;
            }
            match (name.as_str(), is_close) {
                ("code", false) => {
                    let (body, after) = take_until_close(&chars, i, "code");
                    i = after;
                    out.push('`');
                    out.push_str(strip_tags(&body).trim());
                    out.push('`');
                }
                ("strong" | "b", false) => {
                    let (body, after) = take_until_close(&chars, i, &name);
                    i = after;
                    out.push_str("**");
                    out.push_str(&inline_md(&body));
                    out.push_str("**");
                }
                ("em" | "i", false) => {
                    let (body, after) = take_until_close(&chars, i, &name);
                    i = after;
                    out.push('*');
                    out.push_str(&inline_md(&body));
                    out.push('*');
                }
                ("a", false) => {
                    let href = attr_value(&attrs, "href").unwrap_or_default();
                    let (body, after) = take_until_close(&chars, i, "a");
                    i = after;
                    let text = inline_md(&body);
                    if href.is_empty() {
                        out.push_str(&text);
                    } else {
                        out.push('[');
                        out.push_str(text.trim());
                        out.push_str("](");
                        out.push_str(&href);
                        out.push(')');
                    }
                }
                ("br", false) | ("br/", false) => out.push(' '),
                _ => {}
            }
        } else {
            let start = i;
            while i < chars.len() && chars[i] != '<' {
                i += 1;
            }
            let raw: String = chars[start..i].iter().collect();
            out.push_str(&decode_entities(&raw));
        }
    }
    collapse_ws_inline(&out)
}

fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let lower = attrs.to_ascii_lowercase();
    let idx = lower.find(&needle)?;
    let rest = &attrs[idx + needle.len()..];
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let end = rest[1..].find(quote)?;
        Some(decode_entities(&rest[1..1 + end]))
    } else {
        Some(rest.split_whitespace().next().unwrap_or("").to_string())
    }
}

fn table_to_markdown(html: &str) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let (tag, is_close, _, next) = parse_tag(&chars, i);
            i = next;
            let name = tag.to_ascii_lowercase();
            if !is_close && name == "tr" {
                let (body, after) = take_until_close(&chars, i, "tr");
                i = after;
                let cells = table_row_cells(&body);
                if !cells.is_empty() {
                    rows.push(cells);
                }
            }
        } else {
            i += 1;
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut md = String::new();
    for (ri, row) in rows.iter().enumerate() {
        md.push('|');
        for ci in 0..width {
            md.push(' ');
            md.push_str(row.get(ci).map(|s| s.as_str()).unwrap_or(""));
            md.push_str(" |");
        }
        md.push('\n');
        if ri == 0 {
            md.push('|');
            for _ in 0..width {
                md.push_str(" --- |");
            }
            md.push('\n');
        }
    }
    md
}

fn table_row_cells(html: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            let (tag, is_close, _, next) = parse_tag(&chars, i);
            i = next;
            let name = tag.to_ascii_lowercase();
            if !is_close && (name == "td" || name == "th") {
                let (body, after) = take_until_close(&chars, i, &name);
                i = after;
                cells.push(inline_md(&body).trim().replace('|', "\\|"));
            }
        } else {
            i += 1;
        }
    }
    cells
}

fn guess_fence_lang(code: &str) -> &'static str {
    let t = code.trim_start();
    if t.starts_with('{') {
        "json"
    } else if t.starts_with("import ") || t.contains("BarWidget") || t.contains("QtQuick") {
        "qml"
    } else if t.starts_with('#') && t.contains("Custom Clock") {
        "md"
    } else if t.contains("omarchy plugin") || t.contains("PLUGIN_") || t.contains("qmllint") {
        "bash"
    } else if t.starts_with("MIT License") {
        "text"
    } else {
        ""
    }
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some(semi) = rest.find(';') {
            let ent = &rest[..=semi];
            match ent {
                "&amp;" => out.push('&'),
                "&lt;" => out.push('<'),
                "&gt;" => out.push('>'),
                "&quot;" => out.push('"'),
                "&apos;" | "&#39;" | "&rsquo;" => out.push('\''),
                "&nbsp;" => out.push(' '),
                "&mdash;" => out.push_str("—"),
                "&ndash;" => out.push('–'),
                _ if ent.starts_with("&#x") || ent.starts_with("&#X") => {
                    if let Ok(n) = u32::from_str_radix(&ent[3..ent.len() - 1], 16) {
                        if let Some(c) = char::from_u32(n) {
                            out.push(c);
                        }
                    }
                }
                _ if ent.starts_with("&#") => {
                    if let Ok(n) = ent[2..ent.len() - 1].parse::<u32>() {
                        if let Some(c) = char::from_u32(n) {
                            out.push(c);
                        }
                    }
                }
                _ => out.push_str(ent),
            }
            rest = &rest[semi + 1..];
        } else {
            out.push_str(rest);
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

fn collapse_ws_inline(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            prev_space = false;
            out.push(ch);
        }
    }
    out
}

/// Replace runs of 3+ newlines with exactly two.
fn collapse_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut run = 0usize;
    for ch in s.chars() {
        if ch == '\n' {
            run += 1;
            if run <= 2 {
                out.push(ch);
            }
        } else {
            run = 0;
            out.push(ch);
        }
    }
    out
}

/// Split a markdown doc into (heading, body) sections at ## level.
/// The preamble becomes one section titled after fallback_title.
fn split_sections(md_text: &str, fallback_title: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut cur_title = fallback_title.to_string();
    let mut cur: Vec<&str> = Vec::new();

    for line in md_text.lines() {
        if let Some(title) = h2_title(line) {
            flush_section(&mut sections, &cur_title, &cur);
            cur.clear();
            cur_title = title;
        } else {
            cur.push(line);
        }
    }
    flush_section(&mut sections, &cur_title, &cur);
    sections
}

fn flush_section(sections: &mut Vec<(String, String)>, title: &str, cur: &[&str]) {
    let body = cur.join("\n");
    let body = body.trim();
    if !body.is_empty() {
        sections.push((title.to_string(), body.to_string()));
    }
}

/// Returns Some(stripped heading) if the line is `## <whitespace> ...`.
fn h2_title(line: &str) -> Option<String> {
    let rest = line.strip_prefix("##")?;
    if rest.starts_with(char::is_whitespace) {
        Some(rest.trim().to_string())
    } else {
        None
    }
}

/// First line anywhere matching `# <whitespace> ...` (like Python re.search).
fn find_h1(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let rest = line.strip_prefix('#')?;
        if rest.starts_with(char::is_whitespace) {
            Some(rest.trim().to_string())
        } else {
            None
        }
    })
}

/// First line only, mirroring Python re.match(r"^#\s+(.*)$", text) WITHOUT
/// MULTILINE: '$' only matches at end of string (or before one trailing
/// newline), so this effectively only matches single-line documents. Multi-
/// line docs therefore fall back to the file stem, like the original builder.
fn match_h1(text: &str) -> Option<String> {
    let after_hash = text.strip_prefix('#')?;
    let ws_len = after_hash.len() - after_hash.trim_start().len();
    if ws_len == 0 {
        return None;
    }
    let rest = &after_hash[ws_len..];
    let (cap, tail) = match rest.find('\n') {
        None => (rest, ""),
        Some(i) => (&rest[..i], &rest[i..]),
    };
    let anchored = tail.is_empty() || tail == "\n";
    if anchored {
        Some(cap.trim().to_string())
    } else {
        None
    }
}

fn load_url_map(root: &Path) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    if let Ok(text) = fs::read_to_string(root.join(SEARCH_INDEX_FILE)) {
        if let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&text) {
            for e in entries {
                if let (Some(chapter), Some(url)) = (
                    e.get("chapter").and_then(Value::as_str),
                    e.get("url").and_then(Value::as_str),
                ) {
                    out.entry(norm_key(chapter))
                        .or_insert_with(|| format!("https://omarchy.org{url}"));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------- ingestion

struct Ingested<'a> {
    con: &'a Connection,
}

impl Ingested<'_> {
    fn add(
        &self,
        source: &str,
        title: &str,
        section: &str,
        body: &str,
        path: &str,
        url: &str,
        version: Option<&str>,
        published: Option<&str>,
        priority: i64,
    ) {
        let _ = self.con.execute(
            "INSERT INTO chunks(source, title, section, body, path, url, version, published, priority) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                source,
                title,
                section,
                truncate_chars(body, BODY_CAP),
                path,
                url,
                version,
                published,
                priority
            ],
        );
    }

    fn ingest_manual(&self, repo: &Path, root: &Path) -> usize {
        let url_map = load_url_map(root);
        let mut count = 0;
        let manual_dir = repo.join("manual");
        let mut files: Vec<PathBuf> = list_md_files(&manual_dir);
        files.sort();
        for f in files {
            let Ok(raw) = fs::read_to_string(&f) else {
                continue;
            };
            let text = clean_markdown(raw.strip_prefix('\u{feff}').unwrap_or(&raw));
            let title = match find_h1(&text) {
                Some(t) if !t.is_empty() => t,
                _ => f
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            };
            let url = url_map.get(&norm_key(&title)).cloned().unwrap_or_default();
            if url.is_empty() {
                println!(
                    "  ! no live URL for {} ('{}'); indexing without link",
                    file_name(&f),
                    title
                );
            }
            let rel = rel_to_repo(repo, &f);
            for (sec_title, body) in split_sections(&text, &title) {
                let stored = if sec_title == title {
                    body.clone()
                } else {
                    format!("# {}\n\n{}", sec_title, body)
                };
                self.add(
                    "manual", &title, &sec_title, &stored, &rel, &url, None, None, 1,
                );
                count += 1;
            }
        }
        count
    }

    fn ingest_repo_docs(&self, repo: &Path) -> usize {
        let mut count = 0;
        let docs_dir = repo.join("docs");
        let mut files: Vec<PathBuf> = list_md_files(&docs_dir);
        files.sort();
        for f in files {
            let Ok(raw) = fs::read_to_string(&f) else {
                continue;
            };
            let text = clean_markdown(&raw);
            let title = match match_h1(&text) {
                Some(t) if !t.is_empty() => t,
                _ => f
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            };
            let rel = rel_to_repo(repo, &f);
            let url = format!("https://github.com/omacom/omarchy/tree/quattro/{rel}");
            for (sec_title, body) in split_sections(&text, &title) {
                self.add(
                    "repo-docs",
                    &title,
                    &sec_title,
                    &body,
                    &rel,
                    &url,
                    None,
                    None,
                    1,
                );
                count += 1;
            }
        }
        count
    }

    /// Index every default config file as a single chunk keyed by its path.
    fn ingest_configs(&self, repo: &Path) -> usize {
        let mut count = 0;
        for base in ["config", "default", "etc"] {
            let basedir = repo.join(base);
            if !basedir.exists() {
                continue;
            }
            let mut files: Vec<PathBuf> = Vec::new();
            walk_scandir(&basedir, &mut files);
            for f in files {
                let rel = rel_to_repo(repo, &f);
                if rel
                    .split('/')
                    .any(|part| CONFIG_SKIP_DIRS.contains(&part.trim()))
                {
                    continue;
                }
                let Ok(meta) = fs::metadata(&f) else { continue };
                if meta.len() > CONFIG_MAX_BYTES {
                    continue;
                }
                // Skip binaries: first 4 KiB must decode as UTF-8 (like the original).
                let Ok(bytes) = fs::read(&f) else { continue };
                if std::str::from_utf8(&bytes[..bytes.len().min(4096)]).is_err() {
                    continue;
                }
                let name = file_name(&f).to_lowercase();
                let ext = f
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy().to_lowercase()));
                let ext_ok = ext
                    .as_deref()
                    .map(|e| CONFIG_EXTS.contains(&e))
                    .unwrap_or(false);
                if !ext_ok
                    && !CONFIG_NAME_ALLOWLIST.contains(&name.as_str())
                    && !name.ends_with(".conf")
                {
                    continue;
                }
                let Ok(full) = fs::read_to_string(&f) else {
                    continue;
                };
                let body = full.trim();
                if body.is_empty() {
                    continue;
                }
                self.add("config", &rel, "", body, &rel, "", None, None, 2);
                count += 1;
            }
        }
        count
    }

    fn ingest_commits(&self, repo: &Path) -> usize {
        let output = Command::new("git")
            .args([
                "-C",
                &repo.to_string_lossy(),
                "log",
                "--no-merges",
                "--format=%H%x00%as%x00%s%x00%b%x01",
            ])
            .output();
        let raw = match output {
            Ok(o) if o.status.success() => {
                // Python text mode translates \r\n and \r to \n
                // (universal newlines); mirror that before splitting records.
                String::from_utf8_lossy(&o.stdout)
                    .replace("\r\n", "\n")
                    .replace('\r', "\n")
            }
            _ => {
                println!("  ! git history unavailable; skipping commit index");
                return 0;
            }
        };
        let mut count = 0;
        for record in raw.split('\u{1}') {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                continue;
            }
            let parts: Vec<&str> = record.splitn(4, '\u{0}').collect();
            if parts.len() < 3 {
                continue;
            }
            let sha = parts[0];
            let date = parts[1];
            let subject = parts[2];
            let body = if parts.len() == 4 {
                parts[3].trim()
            } else {
                ""
            };
            let content = if body.is_empty() {
                subject.to_string()
            } else {
                format!("{subject}\n\n{body}")
            };
            self.add(
                "commit",
                &format!("commit {}: {subject}", &sha[..sha.len().min(12)]),
                "",
                &content,
                sha,
                &format!("https://github.com/omacom/omarchy/commit/{sha}"),
                None,
                Some(date),
                1,
            );
            count += 1;
        }
        count
    }

    /// Marketplace plugin authoring guides (HTML → markdown sections).
    fn ingest_plugin_guides(&self, root: &Path) -> usize {
        let dir = root.join(PLUGIN_GUIDES_DIR);
        let mut count = 0;
        for &(filename, url) in PLUGIN_GUIDES {
            let path = dir.join(filename);
            let Ok(raw) = fs::read_to_string(&path) else {
                println!("  ! missing {filename}; run scripts/fetch_sources.sh");
                continue;
            };
            let md = html_main_to_markdown(&raw);
            if md.trim().is_empty() {
                println!("  ! empty conversion for {filename}");
                continue;
            }
            let title = match find_h1(&md) {
                Some(t) if !t.is_empty() => t,
                _ => filename.trim_end_matches(".html").to_string(),
            };
            let rel = format!("omarchyplugins/{filename}");
            for (sec_title, body) in split_sections(&md, &title) {
                let stored = if sec_title == title {
                    body.clone()
                } else {
                    format!("# {}\n\n{}", sec_title, body)
                };
                self.add(
                    "plugin-dev",
                    &title,
                    &sec_title,
                    &stored,
                    &rel,
                    url,
                    None,
                    None,
                    1,
                );
                count += 1;
            }
        }
        count
    }

    fn ingest_releases(&self, root: &Path) -> usize {
        let Ok(text) = fs::read_to_string(root.join("data/raw/releases.json")) else {
            return 0;
        };
        let Ok(Value::Array(releases)) = serde_json::from_str::<Value>(&text) else {
            return 0;
        };
        let mut count = 0;
        for r in &releases {
            let version = r
                .get("tag_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim_start_matches('v')
                .to_string();
            let body = r.get("body").and_then(Value::as_str).unwrap_or("");
            if version.is_empty() || body.trim().is_empty() {
                continue;
            }
            let published: String = r
                .get("published_at")
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(10)
                .collect();
            let url = r.get("html_url").and_then(Value::as_str).unwrap_or("");
            for (sec_title, sec_body) in split_release_sections(body) {
                if sec_body.split_whitespace().count() < 5 {
                    continue;
                }
                self.add(
                    "release",
                    &format!("Omarchy v{version}"),
                    &sec_title,
                    &sec_body,
                    "",
                    url,
                    Some(&version),
                    Some(&published),
                    1,
                );
                count += 1;
            }
        }
        count
    }
}

/// Split release-note markdown at `## ` lines; preamble becomes "Overview".
fn split_release_sections(markdown: &str) -> Vec<(String, String)> {
    let mut sections = Vec::new();
    let mut cur_title = "Overview".to_string();
    let mut cur: Vec<&str> = Vec::new();
    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if !cur.is_empty() {
                flush_section(&mut sections, &cur_title, &cur);
            }
            cur.clear();
            cur_title = rest.trim().to_string();
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        flush_section(&mut sections, &cur_title, &cur);
    }
    sections
}

// -------------------------------------------------------------- filesystem

fn list_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().map(|e| e == "md").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out
}

/// os.scandir-style recursive walk: at each directory level, subdirectory
/// names and file names are yielded together in one sorted pass, recursing
/// into a subdirectory immediately when reached. This matches CPython's
/// sorted(rglob("*")) ordering (e.g. `hypr/apps.lua` before `hypr/apps/x.lua`).
fn walk_scandir(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
        Err(_) => return,
    };
    entries.sort();
    for p in entries {
        if p.is_dir() {
            out.push(p.clone());
            walk_scandir(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn rel_to_repo(repo: &Path, f: &Path) -> String {
    f.strip_prefix(repo)
        .unwrap_or(f)
        .to_string_lossy()
        .into_owned()
}

fn file_name(p: &Path) -> &str {
    p.file_name()
        .map(|s| s.to_str().unwrap_or(""))
        .unwrap_or("")
}

/// Short SHA the repo clone is pinned to (mirrors the original builder).
fn repo_head(repo: &Path) -> String {
    let head = repo.join(".git/HEAD");
    let Ok(text) = fs::read_to_string(&head) else {
        return "unknown".into();
    };
    let text = text.trim();
    match text.split_once(": ") {
        Some((_, ref_path)) => fs::read_to_string(repo.join(".git").join(ref_path))
            .map(|s| s.trim().chars().take(12).collect())
            .unwrap_or_else(|_| "unknown".into()),
        None => text.chars().take(12).collect(),
    }
}

// --------------------------------------------------------------------- main

fn main() {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .expect("crate parent")
                .to_path_buf()
        });
    let repo = root.join("data/raw/omarchy-repo");
    if !repo.exists() {
        eprintln!("Run scripts/fetch_sources.sh first.");
        std::process::exit(1);
    }
    let db = root.join("data/kb.sqlite3");
    if let Some(parent) = db.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if db.exists() {
        fs::remove_file(&db).expect("remove old database");
    }

    let con = Connection::open(&db).expect("open database");
    // One transaction for the whole ingest: without it, per-statement
    // autocommit makes ~6200 individual fsyncs (minutes on slow disks).
    con.execute_batch("PRAGMA journal_mode = MEMORY; PRAGMA synchronous = OFF;")
        .expect("set pragmas");
    con.execute_batch(SCHEMA).expect("create schema");

    let ing = Ingested { con: &con };
    con.execute_batch("BEGIN;").expect("begin transaction");
    ing.ingest_manual(&repo, &root);
    ing.ingest_repo_docs(&repo);
    ing.ingest_plugin_guides(&root);
    ing.ingest_configs(&repo);
    ing.ingest_releases(&root);
    ing.ingest_commits(&repo);
    con.execute_batch("COMMIT;").expect("commit transaction");

    let meta_version = fs::read_to_string(root.join("data/raw/releases.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| {
            v.get(0)
                .and_then(|r| r.get("tag_name"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".into());

    con.execute_batch("CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);")
        .expect("meta table");
    let built_at = chrono::Utc::now().format("%Y-%m-%d").to_string();
    for (k, v) in [
        ("latest_release", meta_version.as_str()),
        ("built_from_commit", &repo_head(&repo)),
        ("built_at", built_at.as_str()),
    ] {
        con.execute(
            "INSERT OR REPLACE INTO meta VALUES (?1, ?2)",
            rusqlite::params![k, v],
        )
        .expect("insert meta");
    }

    // Vocabulary table for "did you mean" suggestions on zero-result queries.
    con.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS terms USING fts5vocab(chunks_fts, 'row');",
    )
    .expect("terms table");

    let mut stmt = con
        .prepare("SELECT source, COUNT(*) FROM chunks GROUP BY source")
        .expect("count query");
    let counts: Vec<(String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("count query")
        .filter_map(Result::ok)
        .collect();
    let total: i64 = counts.iter().map(|(_, n)| n).sum();
    println!("Built {}: {} chunks {:?}", file_name(&db), total, counts);
    println!("Latest release pinned: {meta_version}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_image_lines_and_blank_runs() {
        let md = "intro\n\n![logo](x.png)\n\n\ntext\n";
        assert_eq!(clean_markdown(md), "intro\n\ntext");
    }

    #[test]
    fn keeps_inline_images() {
        let md = "see ![pic](x.png) inline\n";
        assert_eq!(clean_markdown(md), "see ![pic](x.png) inline");
    }

    #[test]
    fn splits_h2_sections() {
        // Preamble keeps everything above the first ## heading, like the original.
        let secs = split_sections("# T\n\npre\n## One\n\nalpha\n## Two\nbeta", "T");
        assert_eq!(secs.len(), 3);
        assert_eq!(secs[0], ("T".into(), "# T\n\npre".into()));
        assert_eq!(secs[1], ("One".into(), "alpha".into()));
        assert_eq!(secs[2], ("Two".into(), "beta".into()));
    }

    #[test]
    fn h2_requires_whitespace_after_hashes() {
        assert!(h2_title("## Foo").is_some());
        assert!(h2_title("### Foo").is_none());
        assert!(h2_title("##Foo").is_none());
    }

    #[test]
    fn release_sections_preamble_is_overview() {
        let secs = split_release_sections("top bits\n## Added\n\nstuff here");
        assert_eq!(secs[0].0, "Overview");
        assert_eq!(secs[1].0, "Added");
    }

    #[test]
    fn plugin_guide_html_keeps_headings_code_and_table() {
        let html = r#"<html><main id="main-content">
          <h1>Develop a Custom Plugin</h1>
          <p>Clone with <code>omarchy plugin clone</code>.</p>
          <h2>Define the Plugin Contract</h2>
          <table><tr><th>Plugin kind</th><th>entryPoints</th></tr>
          <tr><td>bar-widget</td><td>barWidget</td></tr></table>
          <pre><code>{"schemaVersion": 1}</code></pre>
        </main></html>"#;
        let md = html_main_to_markdown(html);
        assert!(md.contains("# Develop a Custom Plugin"), "{md}");
        assert!(md.contains("## Define the Plugin Contract"), "{md}");
        assert!(md.contains("`omarchy plugin clone`"), "{md}");
        assert!(md.contains("| bar-widget |"), "{md}");
        assert!(md.contains("schemaVersion"), "{md}");
    }
}
