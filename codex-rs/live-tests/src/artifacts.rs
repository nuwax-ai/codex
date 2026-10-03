//! Artifact persistence: run manifests, artifact pruning and per-tag
//! event logs under `<repo>/logs/live-<vendor>/`.

use super::*;

// ================================================================
// Artifact persistence
// ================================================================

#[path = "../../exec/src/build_source_identity.rs"]
mod source_identity;

#[derive(Debug)]
pub(crate) struct PreparedExec {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
}

impl PreparedExec {
    /// Checks the same canonical file before and after a turn. A later resolver
    /// must not silently choose a different executable than the manifest.
    pub(crate) fn verify_unchanged(&self) -> Result<()> {
        anyhow::ensure!(
            binary_sha256(&self.path)? == self.sha256,
            "codex-exec changed after its receipt was checked: {}",
            self.path.display()
        );
        Ok(())
    }
}

/// Records source-content evidence before provider work and returns the exact
/// canonical executable to use. The receipt is self-reported, not authenticated.
pub(crate) fn write_manifest(
    dir: &Path,
    cfg: &LiveConfig,
    scenario: &str,
    bridge: Option<&str>,
) -> Result<PreparedExec> {
    let root = repo_root().ok_or_else(|| anyhow!("cannot identify repository for exec receipt"))?;
    let current = source_identity::source_identity(&root)
        .map_err(|error| anyhow!("cannot identify current executable sources: {error}"))?;
    let binary = std::fs::canonicalize(codex_exec_binary()?)?;
    let binary_sha256 = binary_sha256(&binary)?;
    let output = source_identity::bounded_output(
        std::process::Command::new(&binary).arg("--build-receipt"),
        Duration::from_secs(5),
        32 * 1024,
        4096,
    );
    let expected = ReceiptExpectation {
        git_sha: &current.git_sha,
        source_sha256: &current.fingerprint,
        target: env!("LIVE_TESTS_BUILD_TARGET"),
        profile: env!("LIVE_TESTS_BUILD_PROFILE"),
        // codex-exec declares no package features. This says nothing about
        // codex-core/rust-rig or workspace feature unification.
        package_features: &[],
    };
    let validation = match output {
        Ok(bytes) => validate_receipt(&bytes, Some(&expected)),
        Err(error) => failed_receipt("unresolved", &error),
    };
    let stable_binary = binary_sha256 == self::binary_sha256(&binary)?;
    let manifest = serde_json::json!({
        "timestamp": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default(),
        "git_rev": current.git_sha,
        "git_dirty": current.dirty,
        "source_sha256": current.fingerprint,
        "source_input_count": current.inputs.len(),
        "binary_path": binary.display().to_string(),
        "binary_sha256": binary_sha256,
        "binary_unchanged_during_receipt": stable_binary,
        "build": {
            "scope": "HARNESS",
            "git_sha": env!("LIVE_TESTS_BUILD_GIT_SHA"),
            "target": env!("LIVE_TESTS_BUILD_TARGET"),
            "profile": env!("LIVE_TESTS_BUILD_PROFILE"),
            "matches_runtime_head": env!("LIVE_TESTS_BUILD_GIT_SHA") == current.git_sha,
        },
        "exec_build": validation,
        "vendor": cfg.vendor,
        "model": cfg.model,
        "scenario": scenario,
        "experimental_bridge": bridge,
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string())?;
    anyhow::ensure!(
        stable_binary,
        "codex-exec changed while reading its receipt"
    );
    anyhow::ensure!(
        validation["source_validated"] == true
            && validation["package_configuration_validated"] == true,
        "codex-exec source/configuration receipt rejected: {}; rebuild the executable with the current sources and the harness target/profile",
        validation["reason"]
    );
    Ok(PreparedExec {
        path: binary,
        sha256: binary_sha256,
    })
}

fn binary_sha256(path: &Path) -> Result<String> {
    use sha2::Digest;
    anyhow::ensure!(
        std::fs::metadata(path)?.is_file(),
        "codex-exec hash input must be a regular file"
    );
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct BuildReceipt {
    format: String,
    version: u32,
    git_sha: String,
    git_dirty: String,
    source: SourceReceipt,
    target: String,
    profile: String,
    features: PackageFeatures,
    dependency_features: DependencyFeatures,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SourceReceipt {
    algorithm: String,
    sha256: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct PackageFeatures {
    scope: String,
    enabled: Vec<String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct DependencyFeatures {
    status: String,
    reason: String,
}

struct ReceiptExpectation<'a> {
    git_sha: &'a str,
    source_sha256: &'a str,
    target: &'a str,
    profile: &'a str,
    package_features: &'a [String],
}

fn failed_receipt(status: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "status": status,
        "reason": reason,
        "source_validated": false,
        "package_configuration_validated": false,
        "build_configuration_validated": false,
        "provenance": "self_reported_untrusted",
    })
}

/// Pure strict validator: callers supply the source/build expectation, allowing
/// offline tests to cover missing Git, shape errors and all identity mismatches.
fn validate_receipt(bytes: &[u8], expected: Option<&ReceiptExpectation<'_>>) -> serde_json::Value {
    if bytes.len() > 32 * 1024 {
        return failed_receipt("invalid", "receipt exceeds output limit");
    }
    let receipt = match serde_json::from_slice::<BuildReceipt>(bytes) {
        Ok(receipt) => receipt,
        Err(_) => return failed_receipt("invalid", "invalid or incomplete receipt JSON"),
    };
    if receipt.git_sha == "unknown" || receipt.source.sha256 == "unknown" {
        return failed_receipt("unresolved", "build-time Git/source identity unavailable");
    }
    let sha = |value: &str, length: usize| {
        value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    let complete = receipt.format == "codex-exec-build-receipt"
        && receipt.version == 1
        && (sha(&receipt.git_sha, 40) || sha(&receipt.git_sha, 64))
        && matches!(receipt.git_dirty.as_str(), "clean" | "dirty")
        && receipt.source.algorithm == source_identity::SOURCE_ALGORITHM
        && sha(&receipt.source.sha256, 64)
        && !receipt.target.is_empty()
        && receipt.target != "unknown"
        && matches!(receipt.profile.as_str(), "debug" | "release")
        && receipt.features.scope == "codex-exec"
        && receipt
            .features
            .enabled
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        && receipt
            .features
            .enabled
            .iter()
            .all(|feature| !feature.is_empty())
        && receipt.dependency_features.status == "unknown"
        && receipt.dependency_features.reason
            == "cargo_build_script_has_no_resolved_dependency_feature_graph";
    if !complete {
        return failed_receipt("invalid", "receipt fields do not satisfy version 1");
    }
    let Some(expected) = expected else {
        return failed_receipt("unresolved", "current Git/source identity unavailable");
    };
    let source_matches =
        receipt.git_sha == expected.git_sha && receipt.source.sha256 == expected.source_sha256;
    let package_matches = receipt.target == expected.target
        && receipt.profile == expected.profile
        && receipt.features.enabled == expected.package_features;
    serde_json::json!({
        "status": if source_matches && package_matches { "source_and_package_validated" } else { "mismatch" },
        "reason": if !source_matches { "source revision/content mismatch" } else if !package_matches { "target/profile/package feature mismatch" } else { "resolved dependency feature graph unknown" },
        "source_validated": source_matches,
        "package_configuration_validated": package_matches,
        "build_configuration_validated": false,
        "provenance": "self_reported_untrusted",
        "receipt": receipt,
    })
}

#[cfg(test)]
#[path = "artifacts_tests.rs"]
mod tests;

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
