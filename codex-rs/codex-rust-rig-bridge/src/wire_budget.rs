//! Final wire guard, after all hosted replay and pause injection. Byte-based
//! estimates are not a provider tokenizer or an accounting claim.

// Fork (2026-11-10): aligned with OpenAI's documented 512 MB request payload
// limit. The gate is an out-of-control guard against serialization blowups,
// not a pre-judgment of third-party gateway body limits — those are learned
// from observed 413 responses (see the gateway-payload log) instead.
const MAX_WIRE_BYTES: usize = 512 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub(crate) struct RigCallContext {
    pub(crate) auth_domain: Option<String>,
    pub(crate) auth_domain_kind: Option<String>,
    pub(crate) context_window_tokens: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WireBudget {
    pub(crate) context_window_tokens: Option<i64>,
    pub(crate) output_tokens: Option<u64>,
    /// Canonical pre-conversion vision estimate for this request's images
    /// (`wire_images::canonical_vision_floor`). Charged as the maximum with
    /// the wire walk's own vision sum, so cross-protocol detail conversion
    /// can only raise the final budget, never lower it — and never add.
    pub(crate) vision_floor_tokens: u64,
}

#[derive(Debug)]
pub(crate) enum WireBudgetExceeded {
    Bytes,
    Context,
    /// Carries the observed `context_window_tokens` and `output_tokens` so the
    /// message names the values that tripped the check (the bare text gave no
    /// way to tell a misconfigured window from an oversized output budget).
    Configuration {
        context_window_tokens: Option<i64>,
        output_tokens: Option<u64>,
    },
}

impl std::fmt::Display for WireBudgetExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes => {
                formatter.write_str("final wire request exceeds the 512 MB byte budget")
            }
            Self::Context => {
                formatter.write_str("final wire request exceeds the estimated model context budget")
            }
            Self::Configuration {
                context_window_tokens,
                output_tokens,
            } => write!(
                formatter,
                "model context must be positive and larger than the configured output budget \
                 (context_window_tokens={context_window_tokens:?}, output_tokens={output_tokens:?})"
            ),
        }
    }
}

impl std::error::Error for WireBudgetExceeded {}

impl WireBudget {
    /// `image_costs` separates wire bytes from model content cost: inline
    /// image payloads are excluded from the text estimate and re-charged as
    /// coarse vision tokens (`wire_images::measure`). `None` keeps the legacy
    /// whole-body text estimate — used when no context window is configured
    /// or the body could not be measured.
    pub(crate) fn check_body(
        self,
        wire_bytes: usize,
        image_costs: Option<crate::wire_images::WireImageCosts>,
    ) -> Result<(), rig_core::http_client::Error> {
        // Fork (2026-11-17): only a non-positive window is a configuration
        // error. `output >= window` used to fail too, but a deployment whose
        // gateway window is smaller than the model's catalog output limit is
        // legitimate (proxy deployments expose a reduced window — e.g.
        // deepseek-flash at 256k on a gateway vs the catalog's 384k output);
        // there the output budget is simply clamped to the window and the
        // context estimate below still guards the request.
        if self.context_window_tokens.is_some_and(|window| window <= 0) {
            return Err(rig_core::http_client::Error::Instance(Box::new(
                WireBudgetExceeded::Configuration {
                    context_window_tokens: self.context_window_tokens,
                    output_tokens: self.output_tokens,
                },
            )));
        }
        let estimate = match image_costs {
            Some(costs) => {
                let text_bytes = wire_bytes.saturating_sub(costs.inline_payload_bytes);
                codex_utils_string::approx_tokens_from_byte_count(text_bytes)
                    .saturating_add(costs.vision_tokens.max(self.vision_floor_tokens))
            }
            None => codex_utils_string::approx_tokens_from_byte_count(wire_bytes),
        };
        let exceeds_context = self.context_window_tokens.is_some_and(|window| {
            u64::try_from(window).map_or(true, |window| {
                estimate.saturating_add(self.output_tokens.unwrap_or_default()) > window
            })
        });
        let failure = if wire_bytes > MAX_WIRE_BYTES {
            Some(WireBudgetExceeded::Bytes)
        } else if exceeds_context {
            Some(WireBudgetExceeded::Context)
        } else {
            None
        };
        if let Some(failure) = failure {
            return Err(rig_core::http_client::Error::Instance(Box::new(failure)));
        }
        Ok(())
    }
}

pub(crate) fn api_error(
    mut error: &(dyn std::error::Error + 'static),
) -> Option<codex_api::ApiError> {
    loop {
        if let Some(failure) = error.downcast_ref::<WireBudgetExceeded>() {
            return Some(match failure {
                WireBudgetExceeded::Bytes => codex_api::ApiError::InvalidRequest {
                    message: failure.to_string(),
                },
                WireBudgetExceeded::Configuration {
                    context_window_tokens: _,
                    output_tokens: _,
                } => codex_api::ApiError::InvalidRequest {
                    message: failure.to_string(),
                },
                WireBudgetExceeded::Context => codex_api::ApiError::ContextWindowExceeded,
            });
        }
        // Rig 0.44's `ProviderError::Http` keeps its transport error in an
        // `Arc` that `source()` does not expose; descend into it explicitly
        // so a failure classified here stays non-retryable when rig wraps it.
        if let Some(rig_core::error::ProviderError::Http(inner)) =
            error.downcast_ref::<rig_core::error::ProviderError>()
        {
            error = &**inner;
            continue;
        }
        let source = error.source()?;
        error = source;
    }
}

#[cfg(test)]
#[path = "wire_budget_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "wire_budget_outbound_tests.rs"]
mod outbound_tests;
