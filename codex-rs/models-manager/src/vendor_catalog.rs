//! Bundled vendor model catalog: a build-time snapshot, fork-curated to the
//! providers nuwax-harness serves.
//!
//! Two sources feed it, and `scripts/refresh_vendor_catalog.py` owns the
//! merge. The **rig-core snapshot we already ship** (`rig-core-*/src/catalog/
//! models.json`) is authoritative for the rows it knows: upgrading rig
//! upgrades the facts, which is why `limit.output`/`modalities` track rig's
//! release cadence instead of a manual models.dev refresh. rig normalizes
//! providers, though, and carries neither Qwen/Alibaba, StepFun, SiliconFlow,
//! Zhipu-official nor the plan-style keys — those stay seeded from models.dev
//! (`--input api.json`) and are simply not refreshed by rig.
//!
//! Provider keys and the whitelist are ours and never rewritten by a refresh;
//! the parser reads only known fields and ignores anything upstream adds or
//! renames, so schema drift degrades to "missing entry" — which the anchor
//! tests turn into a loud failure for the entries we rely on.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ApplyPatchToolType;
use codex_protocol::openai_models::CatalogModel;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelOrigin;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;
use serde::Deserialize;
use serde_json::Value;

/// The committed snapshot; refreshed by the script, never fetched at runtime.
const CATALOG_JSON: &str = include_str!("../vendor/catalog.json");

/// The fork overlay: fork-verified facts models.dev does not carry.
const OVERLAY_TOML: &str = include_str!("../vendor/overlay.toml");

/// Keep this in sync with `SIZE_BUDGET_BYTES` in the refresh script.
/// Audited by the refresh script's anchor check, not read at runtime.
#[allow(dead_code)]
const SIZE_BUDGET_BYTES: usize = 200 * 1024;

/// Parsed overlay (see `vendor/overlay.toml`). Unknown tables/fields are
/// ignored so the overlay can grow without breaking older readers.
#[derive(Debug, Default, Deserialize, Clone, PartialEq)]
pub struct Overlay {
    #[serde(default)]
    pub effort: BTreeMap<String, EffortTable>,
    #[serde(default)]
    pub alias: BTreeMap<String, Alias>,
    #[serde(default)]
    pub provider: BTreeMap<String, ProviderTuning>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct EffortTable {
    pub levels: Vec<String>,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Alias {
    pub provider: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct ProviderTuning {
    #[serde(default)]
    pub effective_context_window_percent: Option<i64>,
}

/// The committed overlay.
pub(crate) fn overlay() -> &'static Overlay {
    static OVERLAY: OnceLock<Overlay> = OnceLock::new();
    OVERLAY.get_or_init(|| {
        toml::from_str::<Overlay>(OVERLAY_TOML)
            .unwrap_or_default()
            .normalized()
    })
}

impl Overlay {
    /// Lowercases every table key. Overlay keys are model slugs and provider
    /// names whose meaningful comparison is case-insensitive (operators write
    /// `GLM-5.3-Flash`, the committed overlay writes `glm-5.3-flash`); without
    /// normalization a merged map would carry both variants and whichever the
    /// exact lookup hits first silently shadows the other.
    fn normalized(self) -> Overlay {
        let lower = |map: &BTreeMap<String, EffortTable>| {
            map.iter()
                .map(|(key, table)| (key.to_ascii_lowercase(), table.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let lower_alias = |map: &BTreeMap<String, Alias>| {
            map.iter()
                .map(|(key, alias)| (key.to_ascii_lowercase(), alias.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        let lower_provider = |map: &BTreeMap<String, ProviderTuning>| {
            map.iter()
                .map(|(key, tuning)| (key.to_ascii_lowercase(), tuning.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        Overlay {
            effort: lower(&self.effort),
            alias: lower_alias(&self.alias),
            provider: lower_provider(&self.provider),
        }
    }

    /// Field-by-field deep merge (Spec §11 decision): every table merges
    /// per key, and within a key each present field wins — an unset field
    /// keeps the base value. `self` is the base, `overlay` overlays it.
    pub fn merged_with(&self, overlay: &Overlay) -> Overlay {
        let mut merged = self.clone();
        let overlay = &overlay.clone().normalized();
        for (slug, table) in &overlay.effort {
            match merged.effort.get_mut(slug) {
                Some(base) => {
                    if !table.levels.is_empty() {
                        base.levels = table.levels.clone();
                    }
                    if table.default.is_some() {
                        base.default = table.default.clone();
                    }
                }
                None => {
                    merged.effort.insert(slug.clone(), table.clone());
                }
            }
        }
        for (slug, alias) in &overlay.alias {
            merged.alias.insert(slug.clone(), alias.clone());
        }
        for (provider, tuning) in &overlay.provider {
            match merged.provider.get_mut(provider) {
                Some(base) => {
                    if tuning.effective_context_window_percent.is_some() {
                        base.effective_context_window_percent =
                            tuning.effective_context_window_percent;
                    }
                }
                None => {
                    merged.provider.insert(provider.clone(), tuning.clone());
                }
            }
        }
        merged
    }
}

/// Parses a user overlay document (models.d file). Formatting errors fail
/// fast with the file's name expected from the caller's context.
pub fn parse_user_overlay(text: &str) -> Result<Overlay, String> {
    toml::from_str::<Overlay>(text)
        .map(Overlay::normalized)
        .map_err(|error| error.to_string())
}

/// The committed overlay merged with an optional user overlay (models.d).
pub(crate) fn overlay_with(user: Option<&Overlay>) -> Overlay {
    match user {
        Some(user) => overlay().merged_with(user),
        None => overlay().clone(),
    }
}

/// The effective effort-tier table for a model slug, after merging the
/// user overlay: the tier gate's catalog default (Spec §5.1).
pub fn effort_table_for(slug: &str, user: Option<&Overlay>) -> Option<EffortTable> {
    overlay_with(user)
        .effort
        .get(&slug.to_ascii_lowercase())
        .cloned()
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    // Present in every models.dev snapshot; retained for fidelity, not read.
    #[serde(rename = "_meta")]
    #[allow(dead_code)]
    meta: Value,
    providers: BTreeMap<String, Value>,
}

/// One model entry from the catalog, exposing only the fields the fork uses.
/// Unknown or malformed fields read as `None`/defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorModel {
    pub provider: String,
    pub slug: String,
    pub display_name: Option<String>,
    /// Prompt/context token limit (`limit.context`).
    pub context_limit: Option<i64>,
    /// Output token limit (`limit.output`).
    pub output_limit: Option<i64>,
    /// Input modalities, e.g. `["text", "image"]`.
    pub modalities: Vec<String>,
    /// models.dev's boolean "this model reasons" flag. Tier *lists* are the
    /// overlay's job — a bare boolean invents no tiers.
    pub reasons: bool,
    /// models.dev's `attachment` flag: the model takes file attachments.
    pub attachment: bool,
    /// models.dev's `tool_call` flag: the model emitted a tool call at least
    /// once upstream.
    pub tool_call: bool,
    /// Where the model writes interleaved reasoning (`reasoning_content`,
    /// `reasoning`, ...). Gateways differ; the catalog records what the
    /// vendor's own API uses.
    pub interleaved_field: Option<String>,
}

/// The catalog facts a request does not act on yet, kept together so
/// `doctor` and future gates read one shape instead of poking at JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogFacts {
    pub provider: String,
    pub context_limit: Option<i64>,
    pub output_limit: Option<i64>,
    pub tool_call: bool,
    pub attachment: bool,
    pub reasons: bool,
    pub interleaved_field: Option<String>,
}

/// The catalog row for `slug` as diagnostics fields, if the catalog has one.
pub fn facts_for(slug: &str) -> Option<CatalogFacts> {
    resolve(slug).map(|entry| CatalogFacts {
        provider: entry.provider,
        context_limit: entry.context_limit,
        output_limit: entry.output_limit,
        tool_call: entry.tool_call,
        attachment: entry.attachment,
        reasons: entry.reasons,
        interleaved_field: entry.interleaved_field,
    })
}

pub(crate) struct VendorCatalog {
    providers: BTreeMap<String, Value>,
    /// lowercase slug -> (provider, original key) pairs, built once at parse
    /// time: models.dev keys are lowercase while operators configure the
    /// vendor's display casing (`GLM-5.3-Flash`).
    by_lowercase_slug: BTreeMap<String, Vec<(String, String)>>,
}

fn value_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}

impl VendorCatalog {
    /// Kept for diagnostics and the next catalog refresh iteration.
    #[allow(dead_code)]
    pub(crate) fn provider_ids(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// All entries for a model slug across whitelisted providers, in
    /// lexicographic provider order. Callers resolve duplicates with the
    /// overlay's aliases first (see `disambiguate` in later work).
    pub(crate) fn entries_for_slug(&self, slug: &str) -> Vec<VendorModel> {
        // models.dev keys are lowercase while operators configure the vendor's
        // display casing (`GLM-5.3-Flash`); the parse-time lowercase index
        // carries both shapes.
        let mut entries = Vec::new();
        for (provider, original) in self
            .by_lowercase_slug
            .get(&slug.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let payload = &self.providers[provider];
            let Some(model) = payload.get("models").and_then(|m| m.get(original)) else {
                continue;
            };
            entries.push(VendorModel {
                provider: provider.clone(),
                slug: slug.to_string(),
                display_name: model
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                context_limit: model
                    .get("limit")
                    .and_then(|limit| value_i64(limit, "context")),
                output_limit: model
                    .get("limit")
                    .and_then(|limit| value_i64(limit, "output")),
                modalities: model
                    .get("modalities")
                    .and_then(|m| m.get("input"))
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                attachment: model
                    .get("attachment")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                tool_call: model
                    .get("tool_call")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                interleaved_field: model
                    .get("interleaved")
                    .and_then(|interleaved| interleaved.get("field"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                reasons: model
                    .get("reasoning")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            });
        }
        entries
    }
}

fn parse(json: &str) -> Option<VendorCatalog> {
    let file: CatalogFile = serde_json::from_str(json).ok()?;
    let mut by_lowercase_slug: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (provider, payload) in &file.providers {
        let Some(models) = payload.get("models").and_then(Value::as_object) else {
            continue;
        };
        for key in models.keys() {
            by_lowercase_slug
                .entry(key.to_ascii_lowercase())
                .or_default()
                .push((provider.clone(), key.clone()));
        }
    }
    Some(VendorCatalog {
        providers: file.providers,
        by_lowercase_slug,
    })
}

/// Disambiguation order for a slug present under several providers
/// (Spec §3.4): an overlay alias wins outright; otherwise plan-style entries
/// (`coding-plan` / `token-plan`) match how the fork's users actually reach
/// the model; otherwise the largest declared window wins, with a warning —
/// never a silent arbitrary pick.
pub(crate) fn resolve_with(slug: &str, user: Option<&Overlay>) -> Option<VendorModel> {
    let catalog = bundled()?;
    let entries = catalog.entries_for_slug(slug);
    if entries.is_empty() {
        return None;
    }
    let merged_overlay = overlay_with(user);
    // Alias keys are lowercase in the committed overlay while operators may
    // configure the vendor's display casing — match like every other table.
    let alias = merged_overlay.alias.get(&slug.to_ascii_lowercase());
    if let Some(alias) = alias {
        if let Some(entry) = entries.iter().find(|e| e.provider == alias.provider) {
            return Some(entry.clone());
        }
        tracing::warn!(
            slug,
            provider = %alias.provider,
            "vendor overlay alias names a provider that has no entry for this model; \
             falling back to disambiguation"
        );
    }
    fn pick_plan_entry(entries: &[VendorModel]) -> Option<&VendorModel> {
        entries
            .iter()
            .find(|e| e.provider.contains("coding-plan") || e.provider.contains("token-plan"))
    }
    if let Some(entry) = pick_plan_entry(&entries) {
        let plan_entries: Vec<&str> = entries
            .iter()
            .filter(|e| e.provider.contains("coding-plan") || e.provider.contains("token-plan"))
            .map(|e| e.provider.as_str())
            .collect();
        if plan_entries.len() > 1 {
            tracing::warn!(
                slug,
                chosen = %entry.provider,
                candidates = ?plan_entries,
                "model slug appears under several plan-style vendors; chose the first \
                 in provider order — add an overlay alias to pin it explicitly"
            );
        }
        return Some(entry.clone());
    }
    let mut best: Option<&VendorModel> = None;
    for entry in &entries {
        let better = match (best, entry.context_limit) {
            (None, _) => true,
            (Some(current), Some(limit)) => limit > current.context_limit.unwrap_or(i64::MIN),
            (Some(_), None) => false,
        };
        if better {
            best = Some(entry);
        }
    }
    let winner = best?;
    if entries.len() > 1 {
        let providers: Vec<&str> = entries.iter().map(|e| e.provider.as_str()).collect();
        tracing::warn!(
            slug,
            chosen = %winner.provider,
            candidates = ?providers,
            "model slug appears under several vendors; chose the largest declared \
             window — add an overlay alias to pin it explicitly"
        );
    }
    Some(winner.clone())
}

/// Resolves with the committed overlay alone.
pub fn resolve(slug: &str) -> Option<VendorModel> {
    resolve_with(slug, None)
}

/// The output cap the bundled catalog states for `slug` (`limit.output`).
///
/// Fork (nuwax-codex, 2026-11-23): the request side used to fall back to
/// "half the context window" whenever neither the prompt nor the provider
/// declared a cap. For a 1M-window model whose real completion limit is
/// 128k (MiMo, DeepSeek, GLM) that half is four times what the gateway
/// accepts, so the whole turn 400s. The catalog has carried the right number
/// all along (`read` → `VendorModel::output_limit`); this exposes it to the
/// request builder.
pub fn output_limit_for(slug: &str) -> Option<i64> {
    resolve(slug)
        .and_then(|entry| entry.output_limit)
        .filter(|limit| *limit > 0)
}

/// The process-wide bundled catalog. A snapshot that fails to parse (upstream
/// schema drift beyond tolerance) yields `None`: callers fall back to the
/// placeholder path, and the anchor tests fail loudly on the same data.
pub(crate) fn bundled() -> Option<&'static VendorCatalog> {
    static CATALOG: OnceLock<Option<VendorCatalog>> = OnceLock::new();
    CATALOG.get_or_init(|| parse(CATALOG_JSON)).as_ref()
}

#[cfg(test)]
#[path = "vendor_catalog_tests.rs"]
mod tests;

/// Builds full model metadata from a vendor-catalog entry, tagged with the
/// vendor provenance: every fallback-keyed behavior — review budgets,
/// mid-turn model switching, reviewer image admission, tool face — treats it
/// as a known model, and the fork's window policy takes explicit overrides in
/// full (see `model_info::with_config_overrides`).
pub fn vendor_model_info(entry: &VendorModel) -> CatalogModel {
    let overlay = overlay();
    let percent = overlay
        .provider
        .get(&entry.provider)
        .and_then(|tuning| tuning.effective_context_window_percent)
        .unwrap_or(95);
    let input_modalities = entry
        .modalities
        .iter()
        .filter_map(|m| match m.as_str() {
            "text" => Some(InputModality::Text),
            "image" => Some(InputModality::Image),
            "audio" => Some(InputModality::Audio),
            _ => None,
        })
        .collect::<Vec<_>>();
    // models.dev carries no tool-call capability flag, so the search tool
    // stays off — the fork does not pretend to know what it cannot see.
    let model = ModelInfo {
        slug: entry.slug.clone(),
        display_name: entry
            .display_name
            .clone()
            .unwrap_or_else(|| entry.slug.clone()),
        description: None,
        default_reasoning_level: None,
        // Effort tiers reach the engine as provider-level defaults (see the
        // catalog plan §5); a models.dev `reasoning: true` boolean invents
        // no tiers here.
        supported_reasoning_levels: Vec::new(),
        shell_type: ConfigShellToolType::UnifiedExec,
        visibility: ModelVisibility::None,
        supported_in_api: true,
        priority: 99,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        available_access_programs: None,
        availability_nux: None,
        upgrade: None,
        model_messages: Some(crate::model_info::local_model_messages(&entry.slug)),
        include_skills_usage_instructions: false,
        include_plugin_usage_instructions: false,
        include_apps_usage_instructions: false,
        supports_reasoning_summary_parameter: true,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        // Fork (nuwax-codex): vendor models get the apply_patch tool. Without
        // it an agent edits files through shell heredocs and Python one-liners,
        // which is where half of the "wrote the wrong file" incidents came
        // from. The freeform patch format is plain text; a model that fumbles
        // it gets a tool error, not a corrupted file.
        apply_patch_tool_type: Some(ApplyPatchToolType::Freeform),
        web_search_tool_type: WebSearchToolType::Text,
        web_search_provider: None,
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_image_detail_original: entry.modalities.iter().any(|m| m == "image"),
        context_window: entry.context_limit,
        max_context_window: entry.context_limit,
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: percent,
        experimental_supported_tools: Vec::new(),
        input_modalities,
        supports_search_tool: false,
        supports_experimental_context: false,
        use_responses_lite: false,
        supports_reasoning_effort_updates: false,
        guardian: None,
        node_repl_auto_review_required: false,
        node_repl_disabled: false,
        auto_review_model_override: None,
        model_specialty: None,
        tool_mode: None,
        multi_agent_version: None,
        multi_agent_reasoning_effort: None,
    };
    CatalogModel {
        model: model.into(),
        origin: ModelOrigin::Vendor {
            provider: entry.provider.clone().into_boxed_str(),
        },
    }
}
