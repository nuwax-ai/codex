//! Fork (nuwax-codex): trusting a project trusts the hooks it declares.
//!
//! Upstream requires a per-hook approval that only a TUI review can write, so
//! an app-server host (ACP, containers) discovered a trusted project's hooks
//! but never registered them — no error, no log. These tests pin the four
//! outcomes an operator can reach: untrusted project (unchanged), trusted
//! project (now registered), explicit bypass (unchanged), and the user layer
//! (unchanged, so the fork's rule widened nothing for user hooks).

use std::fs;

use codex_config::AbsolutePathBuf;
use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirements;
use codex_config::ConfigRequirementsToml;
use codex_config::TomlValue;

use super::HookSource;
use super::HookTrustStatus;
use super::discovery::DiscoveryMode;
use super::discovery::discover_handlers;

/// A stack with one project layer whose `.nuwax-harness/hooks.json` declares
/// a single `PreToolUse` command hook. `trusted` mirrors the trust gate's
/// own decision for the layer, exactly as the loader records it.
fn empty_config() -> TomlValue {
    TomlValue::Table(Default::default())
}

fn project_stack(trusted: bool) -> (tempfile::TempDir, ConfigLayerStack) {
    let temp = tempfile::tempdir().expect("temp dir");
    let project_dir = temp.path().join("checkout");
    let nuwax_dir = project_dir.join(".nuwax-harness");
    fs::create_dir_all(&nuwax_dir).expect("create project config dir");
    fs::write(
        nuwax_dir.join("hooks.json"),
        r#"{
          "hooks": {
            "PreToolUse": [
              { "matcher": "shell", "hooks": [ { "type": "command", "command": "echo project-hook" } ] }
            ]
          }
        }"#,
    )
    .expect("write hooks.json");

    let dot_nuwax = AbsolutePathBuf::try_from(nuwax_dir).expect("absolute path");
    let source = ConfigLayerSource::Project {
        dot_codex_folder: dot_nuwax.clone(),
    };
    let entry = if trusted {
        ConfigLayerEntry::new(source, empty_config())
    } else {
        ConfigLayerEntry::new_disabled(
            source,
            empty_config(),
            "add this checkout as a trusted project to load its hooks".to_string(),
        )
    };
    let stack = ConfigLayerStack::new(
        vec![entry],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("config layer stack");
    (temp, stack)
}

struct Discovered {
    /// What `hooks list` reports: every declaration, trusted or not.
    listed_trust: Vec<HookTrustStatus>,
    /// What the engine actually registers for execution. A trusted entry
    /// that is listed but not registered is exactly the bug being fixed.
    registered: usize,
}

fn discover(stack: &ConfigLayerStack, bypass_hook_trust: bool) -> Discovered {
    let discovered = discover_handlers(
        Some(stack),
        Vec::new(),
        Vec::new(),
        bypass_hook_trust,
        DiscoveryMode::Listing,
    );
    Discovered {
        registered: discovered.handlers.len(),
        listed_trust: discovered
            .hook_entries
            .iter()
            .map(|entry| entry.trust_status)
            .collect(),
    }
}

fn trust_only(result: &Discovered) -> Vec<HookTrustStatus> {
    result.listed_trust.clone()
}

/// Case C/D1 of the repro: an untrusted project's hook never runs. The trust
/// gate disables the whole project layer, so its `hooks.json` is not even
/// discovered — which is why the platform saw no error and no log.
#[test]
fn untrusted_project_hook_is_not_discovered() {
    let (_temp, stack) = project_stack(/*trusted*/ false);
    let discovered = discover(&stack, /*bypass*/ false);
    assert!(discovered.listed_trust.is_empty());
    assert_eq!(discovered.registered, 0);
}

/// Case A → now automatic: the operator trusted the checkout, so the hook it
/// ships is trusted with it — no persisted hash, no TUI step. Listed as
/// trusted *and* registered, which is what makes it actually run.
#[test]
fn trusted_project_hook_is_trusted_and_registered_without_a_review() {
    let (_temp, stack) = project_stack(/*trusted*/ true);
    let discovered = discover(&stack, /*bypass*/ false);
    assert_eq!(trust_only(&discovered), vec![HookTrustStatus::Trusted]);
    assert_eq!(
        discovered.registered, 1,
        "a trusted hook must be registered"
    );
}

/// Case B/D2: the explicit bypass covers a hook that has no approval. It is
/// the wide switch — it does not consult the project's trust at all.
#[test]
fn bypass_still_trusts_a_hook_without_an_approval() {
    let (_temp, stack) = project_stack(/*trusted*/ true);
    // No persisted `trusted_hash` exists for this hook; only the bypass
    // would have registered it before the fork's project rule.
    let discovered = discover(&stack, /*bypass*/ true);
    assert_eq!(trust_only(&discovered), vec![HookTrustStatus::Trusted]);
    assert_eq!(discovered.registered, 1);
}

/// An untrusted project stays outside discovery even with the bypass: the
/// trust gate owns that layer, and the bypass only forgives a missing hook
/// approval. Operators who need it must trust the project (or set the bypass
/// *and* expect the layer to still be skipped).
#[test]
fn bypass_does_not_reach_into_an_untrusted_project_layer() {
    let (_temp, stack) = project_stack(/*trusted*/ false);
    let discovered = discover(&stack, /*bypass*/ true);
    assert!(discovered.listed_trust.is_empty());
    assert_eq!(discovered.registered, 0);
}

/// The fork's rule is scoped to project layers: a user-layer hook without a
/// persisted approval is still untrusted, as upstream.
#[test]
fn user_layer_hook_is_untouched_by_the_project_rule() {
    let temp = tempfile::tempdir().expect("temp dir");
    let user_config = temp.path().join("config.toml");
    fs::write(&user_config, "").expect("write user config");
    let user_file = AbsolutePathBuf::try_from(user_config).expect("absolute path");
    // The hooks file sits beside the user config.toml.
    fs::write(
        temp.path().join("hooks.json"),
        r#"{ "hooks": { "Stop": [ { "matcher": "", "hooks": [ { "type": "command", "command": "echo user-hook" } ] } ] } }"#,
    )
    .expect("write user hooks.json");
    let entry = ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: user_file.clone(),
            profile: None,
        },
        TomlValue::Table(Default::default()),
    );
    let stack = ConfigLayerStack::new(
        vec![entry],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("config layer stack");

    let discovered = discover_handlers(
        Some(&stack),
        Vec::new(),
        Vec::new(),
        /*bypass*/ false,
        DiscoveryMode::Listing,
    );
    assert_eq!(discovered.hook_entries.len(), 1);
    assert_eq!(discovered.hook_entries[0].source, HookSource::User);
    assert_eq!(
        discovered.hook_entries[0].trust_status,
        HookTrustStatus::Untrusted
    );
    // Listed so a review can approve it, but not registered.
    assert_eq!(discovered.handlers.len(), 0);
}
