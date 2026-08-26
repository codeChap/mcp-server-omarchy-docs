//! mcp-server-omarchy-docs — offline Omarchy (Quattro/v4) documentation MCP server.
//!
//! Newline-delimited JSON-RPC over stdio per the MCP spec, backed by a
//! read-only SQLite FTS5 knowledge base (`data/kb.sqlite3`). Rebuild the
//! database with `server-rs/src/bin/build-kb.rs`.

mod kb;
mod rpc;
mod tools;
mod util;

use std::path::PathBuf;

use kb::KnowledgeBase;

const SERVER_NAME: &str = "omarchy-docs";
const SERVER_VERSION: &str = "1.0.0";

pub(crate) fn log(msg: &str) {
    eprintln!("[{SERVER_NAME}] {msg}");
}

/// Resolve the project root (the directory holding data/kb.sqlite3).
/// Preference order: $OMARCHY_KB_ROOT, --kb <dir>, the binary's own directory,
/// then the compile-time path of this crate's parent (the Python layout).
fn project_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("OMARCHY_KB_ROOT") {
        let p = PathBuf::from(p);
        if p.join("data/kb.sqlite3").exists() {
            return Some(p);
        }
    }
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--kb" {
            if let Some(p) = args.next() {
                let p = PathBuf::from(p);
                if p.join("data/kb.sqlite3").exists() {
                    return Some(p);
                }
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|p| p.parent()) {
            if root.join("data/kb.sqlite3").exists() {
                return Some(root.to_path_buf());
            }
        }
    }
    // Fallback for `cargo run` / target-dir binaries inside the original checkout.
    let manifest_parent = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .to_path_buf();
    if manifest_parent.join("data/kb.sqlite3").exists() {
        return Some(manifest_parent);
    }
    None
}

fn open_kb() -> KnowledgeBase {
    let root = match project_root() {
        Some(r) => r,
        None => {
            log("could not locate data/kb.sqlite3; set OMARCHY_KB_ROOT or pass --kb");
            std::process::exit(1);
        }
    };
    let db_path = root.join("data/kb.sqlite3");
    if !db_path.exists() {
        log(&format!(
            "Knowledge base missing at {}. Run scripts/build_kb.py first.",
            db_path.display()
        ));
        std::process::exit(1);
    }
    match KnowledgeBase::open(&db_path) {
        Ok(kb) => {
            log(&format!(
                "ready: {} chunks from {}",
                kb.total_chunks(),
                db_path.display()
            ));
            kb
        }
        Err(e) => {
            log(&format!("cannot open database {}: {e}", db_path.display()));
            std::process::exit(1);
        }
    }
}

fn main() {
    // Fast path for the smoke/differential harnesses: dump tool metadata and exit.
    let argv: Vec<String> = std::env::args().collect();
    if argv.iter().any(|a| a == "--tools-json") {
        println!("{}", serde_json::to_string(&tools::tools_list()).unwrap());
        return;
    }
    if argv.iter().any(|a| a == "--version") {
        println!("{SERVER_NAME} {SERVER_VERSION}");
        return;
    }

    let kb = open_kb();
    rpc::serve(&kb);
}
