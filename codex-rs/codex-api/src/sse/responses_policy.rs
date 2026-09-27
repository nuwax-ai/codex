//! Terminal policy shared by the native and Rig Responses stream decoders.

use super::responses::ResponsesStreamEvent;
use crate::ApiError;
use crate::ResponseStream;
use codex_client::StreamResponse;
use serde_json::Value;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ResponseStreamPolicy {
    Native,
    Strict,
}

impl ResponseStreamPolicy {
    pub(super) fn decode(self, data: &str) -> Result<Option<ResponsesStreamEvent>, ApiError> {
        // Some compatible gateways append Chat's sentinel. It never replaces
        // response.completed, so EOF after only this marker remains an error.
        if self == Self::Strict && data.trim() == "[DONE]" {
            return Ok(None);
        }
        match serde_json::from_str::<ResponsesStreamEvent>(data) {
            Ok(event) if self == Self::Strict && event.kind() == "error" => {
                // Responses permits a top-level error event as well as
                // response.failed. Reuse the existing error classification.
                let value: Value = serde_json::from_str(data).map_err(|error| {
                    ApiError::Stream(format!("malformed responses SSE error: {error}"))
                })?;
                let error = value.get("error").cloned().unwrap_or(value);
                serde_json::from_value(serde_json::json!({
                    "type": "response.failed", "response": { "error": error }
                }))
                .map(Some)
                .map_err(|error| {
                    ApiError::Stream(format!("malformed responses SSE error: {error}"))
                })
            }
            Ok(event) => Ok(Some(event)),
            Err(error) if self == Self::Strict => Err(ApiError::Stream(format!(
                "malformed responses SSE event: {error}"
            ))),
            Err(error) => {
                tracing::debug!(
                    error_category = ?error.classify(),
                    error_line = error.line(),
                    error_column = error.column(),
                    payload_bytes = data.len(),
                    "Failed to parse SSE event"
                );
                Ok(None)
            }
        }
    }

    pub(super) fn missing_event(self, kind: &str) -> Result<(), ApiError> {
        if self == Self::Strict
            && matches!(
                kind,
                "response.created"
                    | "response.output_item.added"
                    | "response.output_item.done"
                    | "response.output_text.delta"
                    | "response.custom_tool_call_input.delta"
                    | "response.reasoning_summary_part.added"
                    | "response.reasoning_summary_text.delta"
                    | "response.reasoning_summary_text.done"
                    | "response.reasoning_text.delta"
                    | "response.completed"
                    | "response.failed"
                    | "response.incomplete"
            )
        {
            return Err(ApiError::Stream(format!(
                "responses event `{kind}` was missing required fields"
            )));
        }
        Ok(())
    }
}

/// Decode a Responses HTTP stream with the shared header/metadata handling,
/// rejecting malformed events and stopping immediately on terminal failures.
pub fn spawn_strict_response_stream(
    response: StreamResponse,
    idle_timeout: Duration,
    turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    super::responses::spawn_response_stream_with_policy(
        response,
        idle_timeout,
        /*telemetry*/ None,
        turn_state,
        ResponseStreamPolicy::Strict,
    )
}
