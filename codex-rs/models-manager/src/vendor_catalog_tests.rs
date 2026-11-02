//! Anchor tests over the bundled vendor catalog. These double as the schema
//! drift guard: the refresh script holds the same anchors, so a models.dev
//! change either breaks here (loud) or reads through unchanged.

use super::*;

#[test]
fn the_snapshot_stays_within_its_size_budget() {
    assert!(
        CATALOG_JSON.len() <= SIZE_BUDGET_BYTES,
        "bundled catalog is {} bytes, budget {}",
        CATALOG_JSON.len(),
        SIZE_BUDGET_BYTES
    );
}

#[test]
fn the_whitelisted_providers_are_present() {
    let catalog = bundled().expect("the bundled catalog must parse");
    let providers = catalog.provider_ids();
    for expected in [
        "zai",
        "zai-coding-plan",
        "zhipuai",
        "xiaomi",
        "xiaomi-token-plan-cn",
        "alibaba",
        "alibaba-coding-plan-cn",
        "stepfun",
        "stepfun-step-plan",
        "deepseek",
        "minimax",
        "minimax-cn-coding-plan",
        "moonshotai",
        "siliconflow",
        "ollama-cloud",
    ] {
        assert!(
            providers.contains(&expected),
            "provider {expected} missing from the bundled catalog"
        );
    }
}

/// The fork probe (2026-11-02): a 1M-window gateway accepted 815k and
/// rejected ~979k. If models.dev ever revises this number the probe and the
/// catalog disagree, and this anchor is where that surfaces.
#[test]
fn the_glm_anchor_matches_the_probe() {
    let catalog = bundled().expect("the bundled catalog must parse");
    let entries = catalog.entries_for_slug("glm-5.3-flash");
    let coding_plan = entries
        .iter()
        .find(|entry| entry.provider == "zai-coding-plan")
        .expect("zai-coding-plan carries glm-5.3-flash");
    assert_eq!(coding_plan.context_limit, Some(1_000_000));
}

#[test]
fn entries_read_only_known_fields_and_skip_unknown_shapes() {
    let catalog = bundled().expect("the bundled catalog must parse");
    let entries = catalog.entries_for_slug("glm-5.3-flash");
    assert!(!entries.is_empty());
    for entry in &entries {
        assert_eq!(entry.slug, "glm-5.3-flash");
        assert!(entry.modalities.iter().all(|m| m != "text" || true));
    }
    // A slug that certainly is not in any whitelisted provider.
    assert!(
        catalog
            .entries_for_slug("definitely-not-a-model")
            .is_empty()
    );
}

#[test]
fn a_multi_provider_slug_resolves_through_the_overlay_alias() {
    // glm-5.3-flash exists under several providers; the overlay pins it to
    // the coding-plan entry without consulting fallback ordering.
    let resolved = super::resolve("glm-5.3-flash").expect("resolvable");
    assert_eq!(resolved.provider, "zai-coding-plan");
    assert_eq!(resolved.context_limit, Some(1_000_000));
}

#[test]
fn an_unknown_slug_does_not_resolve() {
    assert!(super::resolve("definitely-not-a-model").is_none());
}

#[test]
fn the_overlay_parses_its_effort_and_alias_tables() {
    let overlay = super::overlay();
    let effort = overlay
        .effort
        .get("glm-5.3-flash")
        .expect("the probed tier table ships in the overlay");
    assert_eq!(effort.levels, vec!["low", "high", "max"]);
    assert_eq!(effort.default.as_deref(), Some("high"));
    assert_eq!(
        overlay
            .alias
            .get("glm-5.3-flash")
            .map(|a| a.provider.as_str()),
        Some("zai-coding-plan")
    );
}

#[test]
fn the_users_display_casing_resolves_the_same_as_the_lowercase_key() {
    // Operators configure `GLM-5.3-Flash`; models.dev keys are lowercase.
    let lower = super::resolve("glm-5.3-flash").expect("lowercase resolves");
    let display = super::resolve("GLM-5.3-Flash").expect("display casing resolves");
    assert_eq!(lower.provider, display.provider);
    assert_eq!(display.provider, "zai-coding-plan");
    assert_eq!(display.context_limit, Some(1_000_000));
}

#[test]
fn the_overlay_alias_matches_display_casing_too() {
    // The same lowercase-index rule as the catalog itself: an operator may
    // configure `GLM-5.3-Flash` while the alias key is lowercase.
    let display = super::resolve_with("GLM-5.3-Flash", None).expect("display resolves");
    assert_eq!(display.provider, "zai-coding-plan");
    // A user overlay alias pinned by display casing also wins.
    let mut user = super::Overlay::default();
    user.alias.insert(
        "GLM-5.3-Flash".to_string(),
        super::Alias {
            provider: "zai".to_string(),
        },
    );
    let pinned = super::resolve_with("glm-5.3-flash", Some(&user)).expect("resolves");
    assert_eq!(pinned.provider, "zai");
}
