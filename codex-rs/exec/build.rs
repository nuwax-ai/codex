//! Build-time source content receipt. Cargo's dependency feature graph is
//! deliberately unresolved: package-local feature flags cannot describe it.

#[path = "src/build_source_identity.rs"]
mod build_source_identity;

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/build_source_identity.rs");
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let root = manifest.as_ref().and_then(|manifest| {
        build_source_identity::git_output(manifest, &["rev-parse", "--show-toplevel"])
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|path| PathBuf::from(path.trim()))
    });
    let identity = root.as_ref().and_then(|root| {
        let branch = build_source_identity::git_output(root, &["symbolic-ref", "--quiet", "HEAD"])
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        for reference in [
            Some("HEAD"),
            branch.as_deref().map(str::trim),
            Some("packed-refs"),
            Some("index"),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(path) =
                build_source_identity::git_output(root, &["rev-parse", "--git-path", reference])
                && let Ok(path) = String::from_utf8(path)
            {
                println!(
                    "cargo:rerun-if-changed={}",
                    root.join(path.trim()).display()
                );
            }
        }
        // Watch workspace member directories so a newly created untracked
        // module reruns this script, without watching workspace target/.
        for parent in [root.join("codex-rs"), root.clone()] {
            if let Ok(entries) = std::fs::read_dir(&parent) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let workspace_input = parent.ends_with("codex-rs")
                        && !matches!(name, "target" | "tmp" | "logs")
                        && !name.starts_with('.');
                    let root_input = matches!(name, "scripts" | "vendor" | ".cargo");
                    if path.is_dir() && (workspace_input || root_input) {
                        println!("cargo:rerun-if-changed={}", path.display());
                    }
                }
            }
        }
        match build_source_identity::source_identity(root) {
            Ok(identity) => {
                for input in &identity.inputs {
                    println!("cargo:rerun-if-changed={}", input.display());
                }
                Some(identity)
            }
            Err(error) => {
                println!("cargo:warning=executable source identity unresolved: {error}");
                None
            }
        }
    });
    let (sha, fingerprint, dirty) =
        identity
            .as_ref()
            .map_or(("unknown", "unknown", "unknown"), |identity| {
                (
                    identity.git_sha.as_str(),
                    identity.fingerprint.as_str(),
                    if identity.dirty { "dirty" } else { "clean" },
                )
            });
    println!("cargo:rustc-env=EXEC_BUILD_GIT_SHA={sha}");
    println!("cargo:rustc-env=EXEC_BUILD_SOURCE_FINGERPRINT={fingerprint}");
    println!("cargo:rustc-env=EXEC_BUILD_DIRTY={dirty}");
    println!(
        "cargo:rustc-env=EXEC_BUILD_SOURCE_ALGORITHM={}",
        build_source_identity::SOURCE_ALGORITHM
    );
    for name in ["TARGET", "PROFILE"] {
        println!("cargo:rerun-if-env-changed={name}");
        let value = std::env::var(name).unwrap_or_else(|_| "unknown".into());
        println!("cargo:rustc-env=EXEC_BUILD_{name}={value}");
    }
    let mut features: Vec<String> = std::env::vars()
        .filter(|(name, value)| name.starts_with("CARGO_FEATURE_") && value == "1")
        .map(|(name, _)| name.trim_start_matches("CARGO_FEATURE_").to_lowercase())
        .collect();
    features.sort();
    println!("cargo:rustc-env=EXEC_BUILD_FEATURES={}", features.join(","));
}
