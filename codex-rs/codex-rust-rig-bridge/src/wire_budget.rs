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

impl WireBudget {
    /// Tokens the final body is charged, using the same math the check applies.
    pub(crate) fn estimate_tokens(
        &self,
        wire_bytes: usize,
        image_costs: Option<crate::wire_images::WireImageCosts>,
    ) -> u64 {
        match image_costs {
            Some(costs) => {
                let text_bytes = wire_bytes.saturating_sub(costs.inline_payload_bytes);
                codex_utils_string::approx_tokens_from_byte_count(text_bytes)
                    .saturating_add(costs.vision_tokens.max(self.vision_floor_tokens))
            }
            None => codex_utils_string::approx_tokens_from_byte_count(wire_bytes),
        }
    }

    /// Output cap that still fits next to this body, when the current cap does
    /// not. `None` means "keep the current cap" (it fits, or the window cannot
    /// be honoured at all and the check reports that).
    pub(crate) fn tightened_output_tokens(
        &self,
        wire_bytes: usize,
        image_costs: Option<crate::wire_images::WireImageCosts>,
    ) -> Option<u64> {
        let window = u64::try_from(self.context_window_tokens.filter(|w| *w > 0)?).ok()?;
        let cap = self.output_tokens?;
        let estimate = self.estimate_tokens(wire_bytes, image_costs);
        if estimate.saturating_add(cap) <= window {
            return None;
        }
        // 剩余空间 < 1 时无法自洽,交给 check_body 报超窗(可恢复路径会压缩历史)。
        (window > estimate).then(|| window.saturating_sub(estimate).max(1))
    }
}

#[derive(Debug)]
pub(crate) enum WireBudgetExceeded {
    Bytes,
    /// Carries the estimate and window so the message names what tripped the
    /// check: a large history, a large vision floor and a small window all
    /// produce the same bare text otherwise.
    Context {
        estimate_tokens: u64,
        context_window_tokens: Option<i64>,
        output_tokens: Option<u64>,
        wire_bytes: usize,
    },
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
            Self::Bytes => formatter.write_str("final wire request exceeds the 512 MB byte budget"),
            Self::Context {
                estimate_tokens,
                context_window_tokens,
                output_tokens,
                wire_bytes,
            } => write!(
                formatter,
                "final wire request exceeds the estimated model context budget \
                 (estimate_tokens={estimate_tokens}, context_window_tokens={context_window_tokens:?}, \
                 output_tokens={output_tokens:?}, wire_bytes={wire_bytes})"
            ),
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
            return Err(rig_core::http_client::Error::permanent(
                WireBudgetExceeded::Configuration {
                    context_window_tokens: self.context_window_tokens,
                    output_tokens: self.output_tokens,
                },
            ));
        }
        let estimate = self.estimate_tokens(wire_bytes, image_costs);
        // Fork (2026-11-19): the check must use the output cap that is actually
        // sent, not a reservation clamped to the remaining room. Clamping the
        // reservation made the local check agree with itself while the wire
        // still carried `min(requested, window/2)` — e.g. window 1100, input
        // 875, sent cap 550 passes here and is rejected by any gateway that
        // validates input+output. Callers now pass the effective cap (post
        // window/2 clamp and post remaining-room tightening).
        let exceeds_context = self.context_window_tokens.is_some_and(|window| {
            u64::try_from(window).map_or(true, |window| {
                estimate.saturating_add(self.output_tokens.unwrap_or_default()) > window
            })
        });
        let failure = if wire_bytes > MAX_WIRE_BYTES {
            Some(WireBudgetExceeded::Bytes)
        } else if exceeds_context {
            Some(WireBudgetExceeded::Context {
                estimate_tokens: estimate,
                context_window_tokens: self.context_window_tokens,
                output_tokens: self.output_tokens,
                wire_bytes,
            })
        } else {
            None
        };
        if let Some(failure) = failure {
            return Err(rig_core::http_client::Error::permanent(failure));
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
                WireBudgetExceeded::Context {
                    estimate_tokens,
                    context_window_tokens,
                    output_tokens,
                    wire_bytes,
                } => {
                    // Interception point: the mapped error is the bare
                    // `ContextWindowExceeded` variant (its classification drives
                    // the compaction/degrade recovery path, so it must not
                    // change), which cannot carry the measurements. Surface them
                    // here so an operator sees the numbers the budget used.
                    tracing::warn!(
                        estimate_tokens,
                        context_window_tokens = ?context_window_tokens,
                        output_tokens = ?output_tokens,
                        wire_bytes,
                        "wire request exceeded the model context budget"
                    );
                    codex_api::ApiError::ContextWindowExceeded
                }
            });
        }
        // Rig 0.47 splits the transport's own error into Transient and
        // Permanent, each holding a boxed source that `source()` may not
        // surface. Descend into it explicitly, like ProviderError::Http below.
        if let Some(
            rig_core::http_client::Error::Transient(inner)
            | rig_core::http_client::Error::Permanent(inner),
        ) = error.downcast_ref::<rig_core::http_client::Error>()
        {
            let inner: &(dyn std::error::Error + 'static) = inner.as_ref();
            error = inner;
            continue;
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
