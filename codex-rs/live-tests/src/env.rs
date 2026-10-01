//! Vendor/endpoint plumbing shared by every suite: auth fixtures, provider
//! construction and endpoint-or-skip helpers.

use super::*;

// ================================================================
// Configuration
// ================================================================

/// Nearest ancestor of this crate's manifest containing `.git`.
pub fn repo_root() -> Option<PathBuf> {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while dir.pop() {
        if dir.join(".git").exists() {
            return Some(dir);
        }
    }
    None
}

// ================================================================
// Shared fixtures
// ================================================================

/// Bearer-token [`AuthProvider`] backed by the static key from `.env.local`.
pub struct StaticBearerAuth(pub String);

impl AuthProvider for StaticBearerAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        let value = format!("Bearer {}", self.0)
            .parse()
            .expect("valid authorization header value");
        headers.insert(AUTHORIZATION, value);
    }
}

pub fn shared_auth(api_key: &str) -> SharedAuthProvider {
    Arc::new(StaticBearerAuth(api_key.to_string()))
}

pub fn vendor_provider(vendor: &str, base_url: &str) -> Provider {
    Provider {
        name: vendor.to_string(),
        base_url: base_url.to_string(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(120),
        max_output_tokens: None,
    }
}

/// Returns the vendor's Responses endpoint, or `None` after a skip notice
/// when none is configured — never a silent fallback to the chat URL (a
/// missing Responses endpoint must skip, not guess; Step declares none).
pub fn responses_url_or_skip(cfg: &LiveConfig) -> Option<String> {
    if cfg.responses_base_url.is_none() {
        println!(
            "vendor `{}` has no Responses endpoint configured (set LIVE_{}_RESPONSES_URL) — skipping",
            cfg.vendor,
            cfg.vendor.to_uppercase().replace('-', "_")
        );
    }
    cfg.responses_base_url.clone()
}

/// Returns the vendor's Anthropic gateway, or `None` after a skip notice
/// when the vendor does not expose one — never falls back across vendors.
pub fn anthropic_url_or_skip(cfg: &LiveConfig) -> Option<String> {
    if cfg.anthropic_base_url.is_none() {
        println!(
            "vendor `{}` has no Anthropic gateway configured (set LIVE_VENDOR_ANTHROPIC_URL) — skipping",
            cfg.vendor
        );
    }
    cfg.anthropic_base_url.clone()
}

pub fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}
