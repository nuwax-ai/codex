//! Final wire guard, after all hosted replay and pause injection. Byte-based
//! estimates are not a provider tokenizer or an accounting claim.

const MAX_WIRE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct RigCallContext {
    pub(crate) auth_domain: Option<String>,
    pub(crate) auth_domain_kind: Option<String>,
    pub(crate) context_window_tokens: Option<i64>,
}

// Compatibility for free-function callers predating auth-domain metadata.
// The ModelBridge entry supplies its actual domain, including None/unknown.
impl Default for RigCallContext {
    fn default() -> Self {
        Self {
            auth_domain: Some("legacy-unscoped".into()),
            auth_domain_kind: None,
            context_window_tokens: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct WireBudget {
    pub(crate) context_window_tokens: Option<i64>,
    pub(crate) output_tokens: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum WireBudgetExceeded {
    Bytes,
    Context,
    Configuration,
}

impl std::fmt::Display for WireBudgetExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Bytes => "final wire request exceeds the 32 MiB byte budget",
            Self::Context => "final wire request exceeds the estimated model context budget",
            Self::Configuration => {
                "model context must be positive and larger than the configured output budget"
            }
        })
    }
}

impl std::error::Error for WireBudgetExceeded {}

impl WireBudget {
    pub(crate) fn check(self, bytes: usize) -> Result<(), rig_core::http_client::Error> {
        if self.context_window_tokens.is_some_and(|window| {
            window <= 0
                || u64::try_from(window)
                    .is_ok_and(|window| self.output_tokens.is_some_and(|output| output >= window))
        }) {
            return Err(rig_core::http_client::Error::Instance(Box::new(
                WireBudgetExceeded::Configuration,
            )));
        }
        let estimate = codex_utils_string::approx_tokens_from_byte_count(bytes);
        let exceeds_context = self.context_window_tokens.is_some_and(|window| {
            u64::try_from(window).map_or(true, |window| {
                estimate.saturating_add(self.output_tokens.unwrap_or_default()) > window
            })
        });
        let failure = if bytes > MAX_WIRE_BYTES {
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
                WireBudgetExceeded::Configuration => codex_api::ApiError::InvalidRequest {
                    message: failure.to_string(),
                },
                WireBudgetExceeded::Context => codex_api::ApiError::ContextWindowExceeded,
            });
        }
        let source = error.source()?;
        error = source;
    }
}

#[cfg(test)]
#[path = "wire_budget_tests.rs"]
mod tests;
