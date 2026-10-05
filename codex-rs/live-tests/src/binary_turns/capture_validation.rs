//! Checks captured wire identity without persisting or echoing URL credentials.

use anyhow::Context;
use anyhow::Result;
use codex_rust_rig_bridge::RigProtocol;
use http::Uri;
use serde_json::Value;
use serde_json::value::RawValue;
use std::collections::BTreeMap;

enum CapturedBody<'a> {
    Raw(BTreeMap<String, &'a RawValue>),
    Convenience(&'a Value),
}

enum CapturedField<'a> {
    Raw(&'a RawValue),
    Convenience(&'a Value),
}

impl CapturedBody<'_> {
    fn get(&self, name: &str) -> Option<CapturedField<'_>> {
        match self {
            Self::Raw(fields) => fields.get(name).copied().map(CapturedField::Raw),
            Self::Convenience(body) => body.get(name).map(CapturedField::Convenience),
        }
    }
}

impl CapturedField<'_> {
    fn matches_u64(self, expected: u64) -> bool {
        match self {
            Self::Raw(value) => {
                serde_json::from_str::<u64>(value.get()).is_ok_and(|value| value == expected)
            }
            Self::Convenience(value) => value.as_u64() == Some(expected),
        }
    }
}

fn captured_body(attempt: &Value) -> Result<CapturedBody<'_>> {
    let Some(raw) = attempt.get("body_raw") else {
        // Injected runners may provide only the historical convenience view.
        return Ok(CapturedBody::Convenience(&attempt["body"]));
    };
    let raw = raw.as_str().context("captured body_raw must be a string")?;
    // Borrow only top-level members. Unknown schema/history values remain raw,
    // so legal huge numbers neither fail validation nor lose their wire bytes.
    serde_json::from_str(raw)
        .map(CapturedBody::Raw)
        .map_err(|_| anyhow::anyhow!("captured body_raw must be a valid JSON object"))
}

#[derive(Clone, Copy)]
pub(super) enum CaptureRequirement {
    Required,
    Optional,
}

pub(super) fn validate_attempt(
    attempt: &Value,
    base_url: &str,
    wire: RigProtocol,
    model: &str,
) -> Result<()> {
    let expected: Uri = base_url
        .split('#')
        .next()
        .unwrap_or_default()
        .parse()
        .context("invalid expected capture endpoint")?;
    let captured: Uri = attempt["url"]
        .as_str()
        .context("capture has no URL")?
        .parse()
        .context("invalid captured endpoint")?;
    let expected_authority = expected
        .authority()
        .context("expected endpoint has no authority")?;
    // Capture strips userinfo and query values. Compare only routing facts;
    // no raw credential-bearing expected URL enters an error message.
    let expected_authority: http::uri::Authority = expected_authority
        .as_str()
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .parse()
        .context("invalid expected authority")?;
    let actual_authority = captured
        .authority()
        .context("captured endpoint has no authority")?;
    anyhow::ensure!(
        expected
            .scheme_str()
            .zip(captured.scheme_str())
            .is_some_and(|(expected, actual)| expected.eq_ignore_ascii_case(actual))
            && expected_authority
                .host()
                .eq_ignore_ascii_case(actual_authority.host())
            && effective_port(&expected, &expected_authority)
                == effective_port(&captured, actual_authority),
        "captured endpoint does not match configured scheme/authority"
    );
    // Match the pinned SDK's Anthropic suffix normalization before it appends
    // /v1/messages, retaining any gateway namespace prefix.
    let base_path = expected.path().trim_end_matches('/');
    let expected_path = match wire {
        RigProtocol::Responses => format!("{base_path}/responses"),
        RigProtocol::Chat => format!("{base_path}/chat/completions"),
        RigProtocol::Anthropic => {
            let base_path = base_path
                .strip_suffix("/v1/messages")
                .or_else(|| base_path.strip_suffix("/messages"))
                .or_else(|| base_path.strip_suffix("/v1"))
                .unwrap_or(base_path);
            format!("{base_path}/v1/messages")
        }
    };
    anyhow::ensure!(
        captured.path() == expected_path,
        "captured endpoint has the wrong protocol path"
    );
    let body = captured_body(attempt)?;
    let captured_model = match body.get("model") {
        Some(CapturedField::Raw(value)) => serde_json::from_str::<String>(value.get()).ok(),
        Some(CapturedField::Convenience(value)) => value.as_str().map(str::to_owned),
        None => None,
    };
    anyhow::ensure!(
        captured_model.as_deref() == Some(model),
        "captured model does not match configured model"
    );
    Ok(())
}

fn effective_port(uri: &Uri, authority: &http::uri::Authority) -> Option<u16> {
    authority.port_u16().or_else(|| match uri.scheme_str() {
        Some(scheme) if scheme.eq_ignore_ascii_case("http") => Some(80),
        Some(scheme) if scheme.eq_ignore_ascii_case("https") => Some(443),
        Some(_) | None => None,
    })
}

/// What the scene expects on the wire for the output budget. The typed
/// expectation replaces ad-hoc field sniffing: Chat may spell the cap
/// `max_tokens` OR `max_completion_tokens` (endpoints that reject the
/// legacy field), Anthropic REQUIRES `max_tokens` (the bridge's default when
/// no budget is configured), Responses uses `max_output_tokens` and omits it
/// entirely when unset.
#[derive(Clone, Copy)]
pub(super) enum CapExpectation {
    Explicit(u64),
    AnthropicDefault,
    Absent,
}

/// Validates the attempt's output-cap field(s) against the expectation and
/// returns the asserted field name(s) for the evidence record.
pub(super) fn validate_cap(
    attempt: &Value,
    wire: RigProtocol,
    expectation: CapExpectation,
) -> Result<&'static str> {
    let body = captured_body(attempt)?;
    match (wire, expectation) {
        (RigProtocol::Responses, CapExpectation::Explicit(expected)) => {
            anyhow::ensure!(
                body.get("max_output_tokens")
                    .is_some_and(|value| value.matches_u64(expected)),
                "responses output cap must be the configured budget"
            );
            Ok("body.max_output_tokens")
        }
        (RigProtocol::Responses, CapExpectation::Absent) => {
            anyhow::ensure!(
                body.get("max_output_tokens").is_none(),
                "no output cap is configured; responses wire must not invent one"
            );
            Ok("body.max_output_tokens_absent")
        }
        (RigProtocol::Chat, CapExpectation::Explicit(expected)) => {
            let legacy = body.get("max_tokens");
            let modern = body.get("max_completion_tokens");
            let field = match (legacy, modern) {
                (Some(value), None) => ("body.max_tokens", value),
                (None, Some(value)) => ("body.max_completion_tokens", value),
                (Some(_), Some(_)) => anyhow::bail!("chat wire carries both cap fields"),
                (None, None) => anyhow::bail!("chat wire carries no output cap"),
            };
            anyhow::ensure!(
                field.1.matches_u64(expected),
                "chat output cap must be the configured budget"
            );
            Ok(field.0)
        }
        (RigProtocol::Chat, CapExpectation::Absent) => {
            anyhow::ensure!(
                body.get("max_tokens").is_none() && body.get("max_completion_tokens").is_none(),
                "no output cap is configured; chat wire must not invent one"
            );
            Ok("body.max_tokens_absent")
        }
        (RigProtocol::Anthropic, CapExpectation::Explicit(expected)) => {
            anyhow::ensure!(
                body.get("max_tokens")
                    .is_some_and(|value| value.matches_u64(expected)),
                "anthropic max_tokens must be the configured budget"
            );
            Ok("body.max_tokens")
        }
        (RigProtocol::Anthropic, CapExpectation::AnthropicDefault) => {
            anyhow::ensure!(
                body.get("max_tokens")
                    .is_some_and(|value| value
                        .matches_u64(codex_rust_rig_bridge::DEFAULT_ANTHROPIC_MAX_TOKENS)),
                "anthropic max_tokens must be the bridge default when no budget is configured"
            );
            Ok("body.max_tokens")
        }
        // The scene builder rejects these combinations up front; reaching
        // them here means the wire API changed under the scene.
        (RigProtocol::Anthropic, CapExpectation::Absent) => {
            anyhow::bail!("anthropic wire always requires max_tokens")
        }
        (RigProtocol::Responses, CapExpectation::AnthropicDefault) => {
            anyhow::bail!("responses wire has no anthropic default cap")
        }
        (RigProtocol::Chat, CapExpectation::AnthropicDefault) => {
            anyhow::bail!("chat wire has no anthropic default cap")
        }
    }
}
