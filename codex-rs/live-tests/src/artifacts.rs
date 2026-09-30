//! Artifact persistence: run manifests, artifact pruning and per-tag
//! event logs under `<repo>/logs/live-<vendor>/`.

use super::*;

// ================================================================
// Artifact persistence
// ================================================================

/// Records what code produced this run: git revision and dirty state, the
/// resolved binary's path and SHA-256 (a runtime git rev plus mtime cannot
/// prove which build produced the artifacts), and the vendor/scenario — so
/// any artifact directory can be traced back to a build. Written into every
/// binary-level run directory.
pub(crate) fn write_manifest(dir: &Path, cfg: &LiveConfig, scenario: &str, bridge: Option<&str>) {
    let root = repo_root();
    let git_output = |args: &[&str]| {
        root.as_ref()
            .and_then(|root| {
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(args)
                    .output()
                    .ok()
            })
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let git_rev = git_output(&["rev-parse", "--short", "HEAD"]);
    let git_dirty = git_output(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty());
    let binary = codex_exec_binary().ok();
    let binary_path = binary.as_ref().map(|path| path.display().to_string());
    let binary_sha256 = binary.as_deref().and_then(binary_sha256);
    let binary_mtime = binary
        .as_ref()
        .and_then(|p| p.metadata().ok())
        .and_then(|m| m.modified().ok())
        .map(|t| {
            t.duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default()
        });
    let manifest = serde_json::json!({
        "timestamp": SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default(),
        "git_rev": git_rev,
        "git_dirty": git_dirty,
        "binary_path": binary_path,
        "binary_sha256": binary_sha256,
        "binary_mtime_unix": binary_mtime,
        "vendor": cfg.vendor,
        "model": cfg.model,
        "scenario": scenario,
        "experimental_bridge": bridge,
    });
    let _ = std::fs::write(dir.join("manifest.json"), manifest.to_string());
}

/// SHA-256 of the test binary, binding artifacts to an exact build (the
/// equivalent of a build receipt without hooking cargo itself).
fn binary_sha256(path: &Path) -> Option<String> {
    use sha2::Digest;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// Keeps the newest `KEEP_RUNS` run directories (and `KEEP_BRIDGE_LOGS`
/// bridge event logs) so `logs/` cannot grow unboundedly.
const KEEP_RUNS: usize = 25;
const KEEP_BRIDGE_LOGS: usize = 120;

pub(crate) fn prune_artifacts(vendor_dir: PathBuf) {
    fn prune_dir(dir: &Path, keep: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((mtime, path))
            })
            .collect();
        if dirs.len() <= keep {
            return;
        }
        dirs.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime)); // newest first
        for (_, path) in dirs.iter().skip(keep) {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    prune_dir(&vendor_dir, KEEP_RUNS);
    prune_dir(&vendor_dir.join("bridge"), KEEP_BRIDGE_LOGS);
}

pub(crate) fn persist_lines(vendor: &str, subdir: &str, tag: &str, lines: &[String]) {
    let Some(root) = repo_root() else {
        return;
    };
    let dir = root
        .join("logs")
        .join(format!("live-{vendor}"))
        .join(subdir);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let path = dir.join(format!("{tag}-{nonce}.log"));
    if std::fs::write(&path, lines.join("\n") + "\n").is_ok() {
        println!("[artifacts] saved {}", path.display());
    }
}
