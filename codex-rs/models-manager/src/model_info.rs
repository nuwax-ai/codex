use codex_protocol::config_types::Personality;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::openai_models::ConfigShellToolType;
use codex_protocol::openai_models::InputModality;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelMessages;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::TruncationMode;
use codex_protocol::openai_models::TruncationPolicyConfig;
use codex_protocol::openai_models::WebSearchToolType;
use codex_protocol::openai_models::default_input_modalities;

use crate::config::ModelsManagerConfig;
use codex_utils_output_truncation::approx_bytes_for_tokens;
use tracing::debug;

pub const BASE_INSTRUCTIONS: &str = include_str!("../prompt.md");
const PERSONALITY_SECTION_HEADER: &str = "# Personality";

pub fn with_config_overrides(mut model: ModelInfo, config: &ModelsManagerConfig) -> ModelInfo {
    if let Some(context_window) = config.model_context_window {
        if context_window <= 0 {
            tracing::warn!(
                context_window,
                "model_context_window must be a positive token count; ignoring it"
            );
        } else if model.used_fallback_model_metadata
            || matches!(
                model.metadata_source,
                codex_protocol::openai_models::MetadataSource::Vendor { .. }
            )
        {
            // Fork (nuwax-codex): fallback and vendor-catalog models both take
            // an explicit override in full. Fallback metadata carries only a
            // synthetic 272k placeholder, not vendor knowledge; a vendor
            // catalog entry is a *default*, while the operator may hold newer
            // facts (configurable-window deployments, e.g. a server-side 1M
            // upgrade on a catalog-listed 131k model). Raising above the
            // catalog value warns — it is never silently clamped. Official
            // catalog models keep the upstream clamp below. An over-large
            // override self-heals: bridge vendors report the over-window
            // error as `ContextWindowExceeded`, which drives the drop-oldest
            // compact retry instead of wedging the session.
            if let Some(catalog_window) = model.max_context_window
                && catalog_window > 0
                && context_window > catalog_window
                && matches!(
                    model.metadata_source,
                    codex_protocol::openai_models::MetadataSource::Vendor { .. }
                )
            {
                tracing::warn!(
                    model = %model.slug,
                    configured = context_window,
                    catalog_window,
                    "model_context_window exceeds the vendor catalog value; applying                      the configured window — verify the model really accepts it"
                );
            }
            model.context_window = Some(context_window);
            model.max_context_window = Some(context_window);
        } else {
            model.context_window = Some(
                model
                    .max_context_window
                    .map_or(context_window, |max_context_window| {
                        context_window.min(max_context_window)
                    }),
            );
        }
    }
    // Fork (nuwax-codex): a non-positive absolute limit would force compaction
    // every turn; ignore it with a warning (config load already rejects these
    // for the TOML/-c path, but ModelsManagerConfig can be constructed
    // directly by app-server paths). With the invalid value dropped, a
    // configured ratio still derives the limit.
    let absolute_compact_limit = match config.model_auto_compact_token_limit {
        Some(limit) if limit <= 0 => {
            tracing::warn!(
                limit,
                "model_auto_compact_token_limit must be a positive token count; ignoring it"
            );
            None
        }
        other => other,
    };
    if let Some(auto_compact_token_limit) = absolute_compact_limit {
        // Fork (nuwax-codex): an explicit limit is authoritative — it is not
        // capped by the derived ratio anymore — so surface, rather than clamp,
        // a value that reaches the full-window compaction line: routine
        // auto-compaction would never fire before the full-window backstop.
        if let Some(context_window) = model.context_window {
            let percent = i64::from(model.effective_context_window_percent.clamp(0, 100));
            let full_window_line = context_window.saturating_mul(percent) / 100;
            if auto_compact_token_limit >= full_window_line {
                tracing::warn!(
                    auto_compact_token_limit,
                    context_window,
                    effective_context_window_percent = percent,
                    full_window_line,
                    "model_auto_compact_token_limit reaches the model's full-window \
                     compaction line; routine auto-compaction will not trigger before \
                     the full-window backstop"
                );
            }
        }
        model.auto_compact_token_limit = Some(auto_compact_token_limit);
    } else if let Some(ratio) = config.model_auto_compact_ratio {
        // Fork (nuwax-codex): ratio-based threshold derives from the
        // effective context window (the override above already applied), so
        // deployments can say "compact at 80% of the window" without
        // hardcoding per-vendor token counts. An explicit absolute limit
        // always wins; an out-of-range ratio is ignored with a warning
        // rather than silently clamping a typo.
        if !(0.0..=1.0).contains(&ratio) {
            tracing::warn!(
                ratio,
                "model_auto_compact_ratio must be within (0, 1]; ignoring it"
            );
        } else if ratio > 0.0
            && let Some(context_window) = model.context_window
        {
            let derived = (context_window as f64 * ratio) as i64;
            if derived <= 0 {
                // A zero threshold would compact every turn; treat the same
                // as an unusable ratio and keep the model's own limit.
                tracing::warn!(
                    ratio,
                    context_window,
                    "model_auto_compact_ratio derives a zero token limit; ignoring it"
                );
            } else {
                model.auto_compact_token_limit = Some(derived);
            }
        }
    }
    if let Some(token_limit) = config.tool_output_token_limit {
        model.truncation_policy = match model.truncation_policy.mode {
            TruncationMode::Bytes => {
                let byte_limit =
                    i64::try_from(approx_bytes_for_tokens(token_limit)).unwrap_or(i64::MAX);
                TruncationPolicyConfig::bytes(byte_limit)
            }
            TruncationMode::Tokens => {
                let limit = i64::try_from(token_limit).unwrap_or(i64::MAX);
                TruncationPolicyConfig::tokens(limit)
            }
        };
    }

    // Fork (nuwax-codex): `[tools] tool_search = false` advertises this model
    // as lacking the search-tool capability, which routes MCP tools directly
    // into the initial tool list instead of deferring them behind tool_search.
    // Only forced OFF: forcing it ON for a model without wire support would
    // emit a tool the backend cannot answer.
    if config.tool_search_enabled == Some(false) {
        model.supports_search_tool = false;
    }

    if let Some(base_instructions) = &config.base_instructions {
        let model_messages = model.model_messages.get_or_insert_default();
        model_messages.instructions_template = Some(base_instructions.clone());
    } else if config.personality == Some(Personality::None)
        && let Some(instructions_template) = model
            .model_messages
            .as_mut()
            .and_then(|messages| messages.instructions_template.as_mut())
    {
        *instructions_template = strip_personality_section(std::mem::take(instructions_template));
    }

    model
}

fn strip_personality_section(mut instructions: String) -> String {
    let mut section_start = None;
    let mut section_end = None;
    let mut offset = 0;

    for line_with_ending in instructions.split_inclusive('\n') {
        let line = match line_with_ending.strip_suffix('\n') {
            Some(line) => line.strip_suffix('\r').unwrap_or(line),
            None => line_with_ending,
        };
        if section_start.is_some() {
            if is_h1_heading(line) {
                section_end = Some(offset);
                break;
            }
        } else if line == PERSONALITY_SECTION_HEADER {
            section_start = Some(offset);
        }
        offset += line_with_ending.len();
    }

    if let Some(section_start) = section_start {
        let section_end = section_end.unwrap_or(instructions.len());
        instructions.replace_range(section_start..section_end, "");
    }

    instructions
}

fn is_h1_heading(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('#') else {
        return false;
    };
    rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')
}

/// Build a minimal fallback model descriptor for missing/unknown slugs.
/// Builds full `ModelInfo` from a vendor-catalog entry: the model behaves
/// like a cataloged one (`used_fallback_model_metadata = false`), so every
/// fallback-keyed behavior — review budgets, mid-turn model switching,
/// reviewer image admission, tool face — treats it as a known model.
pub fn vendor_model_info(entry: &crate::vendor_catalog::VendorModel) -> ModelInfo {
    let overlay = crate::vendor_catalog::overlay();
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
    ModelInfo {
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
        model_messages: Some(local_model_messages(&entry.slug)),
        include_skills_usage_instructions: false,
        include_plugin_usage_instructions: false,
        include_apps_usage_instructions: false,
        supports_reasoning_summary_parameter: true,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_image_detail_original: entry.modalities.iter().any(|m| m == "image"),
        context_window: entry.context_limit,
        max_context_window: entry.context_limit,
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: percent,
        experimental_supported_tools: Vec::new(),
        input_modalities,
        used_fallback_model_metadata: false,
        metadata_source: codex_protocol::openai_models::MetadataSource::Vendor {
            provider: entry.provider.clone().into_boxed_str(),
        },
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
    }
}

/// Warns, once per process per slug, that a model resolved to fallback
/// placeholder metadata. The placeholder window (272k) silently budgeting a
/// session was invisible to users; the warning names the knob to set.
pub fn model_info_from_slug(slug: &str) -> ModelInfo {
    static WARNED_SLUGS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let warned = WARNED_SLUGS.get_or_init(Default::default);
    if let Ok(mut seen) = warned.lock()
        && seen.insert(slug.to_string())
    {
        tracing::warn!(
            slug,
            placeholder_context_window = 272_000,
            "model `{slug}` is not in any catalog; using fallback placeholder \
             metadata with a 272,000-token context window — set \
             `model_context_window` (or NUWAX_MODEL_CONTEXT_WINDOW) to the \
             model's real window so budgets and compaction are derived from it"
        );
    }
    debug!("Unknown model {slug} is used. This will use fallback model metadata.");
    ModelInfo {
        slug: slug.to_string(),
        display_name: slug.to_string(),
        description: None,
        default_reasoning_level: None,
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
        model_messages: Some(local_model_messages(slug)),
        include_skills_usage_instructions: false,
        include_plugin_usage_instructions: false,
        include_apps_usage_instructions: false,
        supports_reasoning_summary_parameter: true,
        default_reasoning_summary: ReasoningSummary::Auto,
        support_verbosity: false,
        default_verbosity: None,
        apply_patch_tool_type: None,
        web_search_tool_type: WebSearchToolType::Text,
        truncation_policy: TruncationPolicyConfig::bytes(/*limit*/ 10_000),
        supports_image_detail_original: false,
        context_window: Some(272_000),
        max_context_window: Some(272_000),
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: 95,
        experimental_supported_tools: Vec::new(),
        input_modalities: default_input_modalities(),
        used_fallback_model_metadata: true, // this is the fallback model metadata
        metadata_source: codex_protocol::openai_models::MetadataSource::Fallback,
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
    }
}

/// Fork (nuwax-codex): unknown models render the bundled template with their
/// own slug in the identity line, so the model reports what it actually is
/// (e.g. "GLM-5.3-Flash running in nuwax-harness") instead of inheriting the
/// upstream Codex/OpenAI identity, which third-party vendor models were never
/// told apart from and would happily claim as their own.
fn local_model_messages(slug: &str) -> ModelMessages {
    ModelMessages {
        instructions_template: Some(fallback_instructions_template(slug)),
        ..Default::default()
    }
}

/// Renders [`BASE_INSTRUCTIONS`] with the `{{model}}` identity placeholder
/// replaced by the model's slug. The placeholder appears exactly once, in the
/// first line.
fn fallback_instructions_template(slug: &str) -> String {
    BASE_INSTRUCTIONS.replace("{{model}}", slug)
}

/// True when `text` is [`BASE_INSTRUCTIONS`] rendered for some slug. Used by
/// test-snapshot normalization to recognize fallback instructions without
/// knowing the slug. The placeholder occupies the first line, so the rendered
/// slug must be non-empty and newline-free; braces are rejected so the
/// unreplaced `{{model}}` literal itself does not count as a slug.
pub fn is_fallback_model_instructions(text: &str) -> bool {
    let Some((prefix, suffix)) = BASE_INSTRUCTIONS.split_once("{{model}}") else {
        return false;
    };
    if !text.starts_with(prefix) || !text.ends_with(suffix) {
        return false;
    }
    let slug = &text[prefix.len()..text.len() - suffix.len()];
    !slug.is_empty() && !slug.contains(['{', '}', '\n'])
}

#[cfg(test)]
#[path = "model_info_tests.rs"]
mod tests;
