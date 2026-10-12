//! Executable self-reported source content and package configuration.
//! Dependency features remain unknown; this is not a signed attestation.

use serde_json::Value;
use serde_json::json;

const GIT_SHA: &str = env!("EXEC_BUILD_GIT_SHA");
const DIRTY: &str = env!("EXEC_BUILD_DIRTY");
const TARGET: &str = env!("EXEC_BUILD_TARGET");
const PROFILE: &str = env!("EXEC_BUILD_PROFILE");
const FEATURES: &str = env!("EXEC_BUILD_FEATURES");
const SOURCE_FINGERPRINT: &str = env!("EXEC_BUILD_SOURCE_FINGERPRINT");
const TREE_STATUS: &str = env!("EXEC_BUILD_TREE_STATUS");

/// The receipt as a JSON object; the `--build-receipt` output shape.
pub fn value() -> Value {
    let tree_status = serde_json::from_str::<Value>(TREE_STATUS)
        .unwrap_or_else(|_| Value::String(TREE_STATUS.to_string()));
    json!({
        "format": "codex-exec-build-receipt",
        "version": 1,
        "git_sha": GIT_SHA,
        "git_dirty": DIRTY,
        "source": {
            "algorithm": env!("EXEC_BUILD_SOURCE_ALGORITHM"),
            "sha256": SOURCE_FINGERPRINT,
        },
        "tree_status": tree_status,
        "target": TARGET,
        "profile": PROFILE,
        "features": {
            "scope": "codex-exec",
            "enabled": FEATURES.split(',').filter(|f| !f.is_empty())
                .map(str::to_string).collect::<Vec<_>>(),
        },
        "dependency_features": {
            "status": "unknown",
            "reason": "cargo_build_script_has_no_resolved_dependency_feature_graph",
        },
    })
}
