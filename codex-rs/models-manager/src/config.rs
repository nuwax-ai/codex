use codex_protocol::config_types::Personality;
use codex_protocol::openai_models::ModelsResponse;

#[derive(Debug, Clone, Default)]
pub struct ModelsManagerConfig {
    pub model_context_window: Option<i64>,
    pub model_auto_compact_token_limit: Option<i64>,
    /// Fork (nuwax-codex): fraction of the effective context window that
    /// derives `auto_compact_token_limit` when no absolute limit is set.
    pub model_auto_compact_ratio: Option<f64>,
    pub tool_output_token_limit: Option<usize>,
    pub base_instructions: Option<String>,
    pub personality: Option<Personality>,
    pub model_catalog: Option<ModelsResponse>,
}
