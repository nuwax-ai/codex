//! Aggregates `logs/live-<vendor>/` into a browsable `INDEX.md` per vendor:
//! every run with its manifest (git rev, binary mtime, scenario) linked to
//! its artifacts, plus the newest bridge event logs.
//!
//! Run any time:
//! ```text
//! cargo run -p codex-live-tests --bin index-logs
//! ```

use std::path::PathBuf;

fn main() {
    let Some(root) = repo_root() else {
        eprintln!("cannot locate repository root");
        std::process::exit(1);
    };
    let logs = root.join("logs");
    let Ok(vendors) = std::fs::read_dir(&logs) else {
        eprintln!("no logs/ directory at {}", logs.display());
        return;
    };
    for vendor in vendors.flatten() {
        let path = vendor.path();
        if !path.is_dir() || !vendor.file_name().to_string_lossy().starts_with("live-") {
            continue;
        }
        write_index(&path);
    }
    println!("done");
}

fn repo_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn write_index(vendor_dir: &std::path::Path) {
    let mut runs: Vec<(String, serde_json::Value)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(vendor_dir) {
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("manifest.json");
            let Ok(raw) = std::fs::read_to_string(&manifest_path) else {
                continue;
            };
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
                runs.push((entry.file_name().to_string_lossy().to_string(), value));
            }
        }
    }
    // newest first by timestamp when present
    runs.sort_by(|a, b| {
        let ta = a.1["timestamp"].as_u64().unwrap_or(0);
        let tb = b.1["timestamp"].as_u64().unwrap_or(0);
        tb.cmp(&ta)
    });

    let mut index = String::new();
    index.push_str("# Live-test artifacts\n\n");
    index.push_str("Regenerate with `cargo run -p codex-live-tests --bin index-logs`.\n\n");
    index.push_str("## Binary-level runs\n\n");
    index.push_str("| run | timestamp | git rev | scenario | bridge | model |\n");
    index.push_str("|---|---|---|---|---|---|\n");
    for (name, m) in &runs {
        let ts = m["timestamp"].as_u64().unwrap_or(0);
        let rev = m["git_rev"].as_str().unwrap_or("?");
        let scenario = m["scenario"].as_str().unwrap_or("?");
        let bridge = m["experimental_bridge"].as_str().unwrap_or("(default)");
        let model = m["model"].as_str().unwrap_or("?");
        index.push_str(&format!(
            "| [{name}](./{name}/events.jsonl) | {ts} | {rev} | {scenario} | {bridge} | {model} |\n"
        ));
    }

    // newest bridge logs
    let bridge_dir = vendor_dir.join("bridge");
    if let Ok(entries) = std::fs::read_dir(&bridge_dir) {
        let mut logs: Vec<_> = entries.flatten().collect();
        logs.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
        index.push_str("\n## Bridge event logs (newest 20)\n\n");
        for entry in logs.iter().take(20) {
            let name = entry.file_name().to_string_lossy().to_string();
            index.push_str(&format!("- [{name}](./bridge/{name})\n"));
        }
    }

    let out = vendor_dir.join("INDEX.md");
    match std::fs::write(&out, index) {
        Ok(()) => println!("wrote {}", out.display()),
        Err(err) => eprintln!("failed to write {}: {err}", out.display()),
    }
}
