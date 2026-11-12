//! Fork (nuwax-codex): OpenAI first-party egress is closed unless explicitly
//! enabled.
//!
//! This fork ships for vendor models reached through the operator's own
//! gateway. A customer who finds `chatgpt.com` in a packet capture — or their
//! own hostname and `installation_id` in our logs — has found a trust
//! incident, not a cosmetic warning. The upstream features that dial OpenAI on
//! their own therefore pass through this gate, and the gate starts closed:
//!
//! - app-server remote control (websocket to `chatgpt.com/backend-api/`)
//! - the plugin directory, featured-plugin ids, and the curated plugin sync
//!   (`chatgpt.com/backend-api/plugins/*`, `api.github.com`,
//!   `github.com/openai/plugins`)
//!
//! Opt in with `NUWAX_OPENAI_EGRESS=1`. `CODEX_OPENAI_EGRESS` is accepted as an
//! alias; when both are set `NUWAX_` wins, matching the rest of the fork's
//! alias policy. Any other non-empty value is a typo, not an opt-in: it is
//! reported once and treated as disabled.

use tracing::warn;

/// Canonical opt-in switch for OpenAI first-party egress.
pub const OPENAI_EGRESS_ENV: &str = "NUWAX_OPENAI_EGRESS";

/// `CODEX_`-prefixed alias, for parity with the fork's other env knobs.
pub const OPENAI_EGRESS_ALIAS_ENV: &str = "CODEX_OPENAI_EGRESS";

/// Whether the operator explicitly allowed OpenAI first-party egress.
///
/// Default `false`: nothing in the harness may contact ChatGPT/OpenAI
/// infrastructure on its own.
pub fn openai_egress_enabled() -> bool {
    env_flag(OPENAI_EGRESS_ENV)
        .or_else(|| env_flag(OPENAI_EGRESS_ALIAS_ENV))
        .unwrap_or(false)
}

/// Parses one env flag. `None` means the variable is unset, so the caller can
/// fall through to the alias; an unparsable value is reported and read as
/// `false` (deny by default) rather than silently enabling egress.
fn env_flag(name: &str) -> Option<bool> {
    let raw = std::env::var(name).ok()?;
    match parse_flag(&raw) {
        Some(enabled) => Some(enabled),
        None => {
            warn!(
                variable = name,
                value = %raw,
                "ignoring unrecognized OpenAI egress flag value; use `1` to enable egress"
            );
            Some(false)
        }
    }
}

fn parse_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "" | "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_documented_spellings() {
        for raw in ["1", "true", "TRUE", " yes ", "on"] {
            assert_eq!(parse_flag(raw), Some(true), "{raw}");
        }
        for raw in ["0", "false", "off", "no", ""] {
            assert_eq!(parse_flag(raw), Some(false), "{raw}");
        }
    }

    #[test]
    fn rejects_anything_that_is_not_an_opt_in() {
        for raw in ["2", "enabled", "y", "chatgpt"] {
            assert_eq!(parse_flag(raw), None, "{raw}");
        }
    }

    #[test]
    fn egress_is_closed_when_neither_variable_is_set() {
        if std::env::var_os(OPENAI_EGRESS_ENV).is_none()
            && std::env::var_os(OPENAI_EGRESS_ALIAS_ENV).is_none()
        {
            assert!(!openai_egress_enabled());
        }
    }
}
