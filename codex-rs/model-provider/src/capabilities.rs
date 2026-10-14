//! Provider capability defaults and resolution of custom provider overrides.

use codex_api::is_azure_responses_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::RemoteCompactionSupport;

/// Optional provider-backed features that Codex may expose at runtime.
///
/// These capabilities are a provider-owned upper bound. Model and turn settings
/// can further restrict the functionality exposed to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub namespace_tools: bool,
    pub image_generation: bool,
    pub web_search: bool,
    pub external_web_access: bool,
    /// Whether the provider can serve the hosted web_search tool in its
    /// CACHED mode. Fork: chat-family bridge wires cannot express it
    /// (Anthropic speaks live search only; Chat has no hosted tools), so the
    /// DEFAULT cached tool must fail closed to disabled there instead of
    /// being advertised and rejected per request.
    pub cached_web_search: bool,
    pub remote_compaction: RemoteCompactionSupport,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            namespace_tools: true,
            image_generation: true,
            web_search: true,
            external_web_access: true,
            cached_web_search: true,
            remote_compaction: RemoteCompactionSupport::Unsupported,
        }
    }
}

impl ProviderCapabilities {
    pub(crate) fn from_config(info: &ModelProviderInfo) -> Self {
        let defaults = Self {
            remote_compaction: if info.is_openai()
                || is_azure_responses_provider(&info.name, info.base_url.as_deref())
            {
                RemoteCompactionSupport::V2
            } else {
                RemoteCompactionSupport::Unsupported
            },
            ..Self::default()
        };
        let overrides = info.capabilities.unwrap_or_default();
        // Fork: the chat-family bridges drop `compaction_trigger` items and
        // cannot express the cached web_search mode, so those capabilities
        // exist only on the native Responses transport regardless of what a
        // custom provider's config declares. Config overrides may lower a
        // capability, never raise it past the wire.
        let native_transport = !info.uses_model_bridge();
        Self {
            external_web_access: overrides
                .external_web_access
                .unwrap_or(defaults.external_web_access),
            cached_web_search: native_transport,
            remote_compaction: if native_transport {
                overrides
                    .remote_compaction
                    .unwrap_or(defaults.remote_compaction)
            } else {
                RemoteCompactionSupport::Unsupported
            },
            ..defaults
        }
    }
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod tests;
