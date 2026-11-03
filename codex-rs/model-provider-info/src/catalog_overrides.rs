//! Fork (nuwax-codex): provider-level opt-outs for catalog-driven defaults,
//! kept in a fork-owned side table instead of fields on upstream structs.
//!
//! Why a side table: the vendor-catalog effort-tier default (Spec §5.3) is a
//! per-provider *configuration* flag, not data about a provider's wire shape.
//! Carrying it as a struct field fanned one boolean out to every construction
//! site of `ModelProviderInfo` and `Provider` — dozens of upstream-owned test
//! fixtures — which every upstream merge then tripped over. A lookup keyed by
//! provider id keeps the flag in exactly one fork-owned file.
//!
//! Source of truth: `model_providers.<id>.ignore_catalog_effort_levels =
//! true` in config.toml (see `config/src/hookup` below) — loaded once at
//! config load into this registry.

use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::sync::RwLock;

fn registry() -> &'static RwLock<BTreeSet<String>> {
    static REGISTRY: OnceLock<RwLock<BTreeSet<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(BTreeSet::new()))
}

/// Records that `provider` opts out of catalog-provided defaults (currently:
/// the vendor-catalog effort-tier default). Idempotent.
pub fn set_ignores_catalog_defaults(provider: &str) {
    if let Ok(mut set) = registry().write() {
        set.insert(provider.to_string());
    }
}

/// True when `provider` opted out of catalog-provided defaults.
pub fn ignores_catalog_defaults(provider: &str) -> bool {
    registry().read().is_ok_and(|set| set.contains(provider))
}
