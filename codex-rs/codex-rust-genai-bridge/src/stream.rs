use codex_api::ApiError;
use codex_api::Provider;
use codex_api::ResponseStream;
use codex_api::ResponsesApiRequest;
use codex_api::SharedAuthProvider;
use codex_api::TransportError;
use futures::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::ChatOptions;
use genai::chat::ChatResponseFormat;
use genai::chat::JsonSpec;
use genai::chat::ToolChoice;
use genai::chat::Verbosity;
use http::HeaderMap;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::convert_request::responses_request_to_chat_request;
use crate::convert_response::chat_event_to_response_event;
use crate::resolver::build_extra_headers;
use crate::resolver::build_genai_client;
use crate::types::PendingAssistantMessage;

const RESPONSE_STREAM_CHANNEL_CAPACITY: usize = 256;

/// The main entry point: converts a Codex `ResponsesApiRequest` into a genai
/// `ChatRequest`, streams via rust-genai, and converts each `ChatStreamEvent`
/// back into a Codex `ResponseEvent` stream.
pub async fn stream_via_genai(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
    adapter_kind: AdapterKind,
    idle_timeout: Duration,
) -> Result<ResponseStream, ApiError> {
    let chat_request = match responses_request_to_chat_request(request) {
        Some(req) => req,
        None => {
            return Err(ApiError::InvalidRequest {
                message: "No convertible messages in request".into(),
            });
        }
    };

    let model = request.model.clone();
    let chat_options = build_chat_options(request, api_provider, api_auth, extra_headers);

    let genai_client = build_genai_client(api_provider, api_auth, adapter_kind);

    let msg_count = chat_request.messages.len();
    let system_present = chat_request.system.is_some();
    tracing::info!(
        model = %model,
        adapter = ?adapter_kind,
        message_count = msg_count,
        has_system_prompt = system_present,
        has_reasoning_effort = chat_options.reasoning_effort.is_some(),
        "Dispatching chat stream via genai"
    );

    // Debug: log each message's role and whether it carries reasoning_content
    for (i, msg) in chat_request.messages.iter().enumerate() {
        let has_reasoning = msg
            .content
            .parts()
            .iter()
            .any(|p| matches!(p, genai::chat::ContentPart::ReasoningContent(_)));
        let part_types: Vec<&str> = msg
            .content
            .parts()
            .iter()
            .map(|p| match p {
                genai::chat::ContentPart::Text(_) => "text",
                genai::chat::ContentPart::ReasoningContent(_) => "reasoning",
                genai::chat::ContentPart::ToolCall(_) => "tool_call",
                genai::chat::ContentPart::ToolResponse(_) => "tool_response",
                genai::chat::ContentPart::ThoughtSignature(_) => "thought_sig",
                genai::chat::ContentPart::Binary(_) => "binary",
                genai::chat::ContentPart::Custom(_) => "custom",
            })
            .collect();
        tracing::debug!(
            msg_index = i,
            role = ?msg.role,
            has_reasoning_content = has_reasoning,
            part_types = ?part_types,
            "Chat message detail"
        );
    }

    let chat_stream_response = genai_client
        .exec_chat_stream(&model, chat_request, Some(&chat_options))
        .await
        .map_err(|e| {
            tracing::error!(
                model = %model,
                error = %e,
                "genai stream failed"
            );
            ApiError::Transport(TransportError::Network(format!("genai stream error: {e}")))
        })?;

    let mut chat_stream = chat_stream_response.stream;

    let (tx, rx) = mpsc::channel(RESPONSE_STREAM_CHANNEL_CAPACITY);

    tokio::spawn(async move {
        let mut pending = PendingAssistantMessage::new();

        loop {
            match tokio::time::timeout(idle_timeout, chat_stream.next()).await {
                Ok(Some(Ok(event))) => {
                    let events = chat_event_to_response_event(event, &mut pending);
                    for ev in events {
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                Ok(Some(Err(e))) => {
                    let _ = tx
                        .send(Err(ApiError::Transport(TransportError::Network(format!(
                            "genai stream error: {e}"
                        )))))
                        .await;
                    return;
                }
                Ok(None) => return,
                Err(_elapsed) => {
                    let _ = tx
                        .send(Err(ApiError::Transport(TransportError::Timeout)))
                        .await;
                    return;
                }
            }
        }
    });

    Ok(ResponseStream {
        rx_event: rx,
        upstream_request_id: None,
    })
}

fn build_chat_options(
    request: &ResponsesApiRequest,
    api_provider: &Provider,
    api_auth: &SharedAuthProvider,
    extra_headers: HeaderMap,
) -> ChatOptions {
    let mut options = ChatOptions {
        capture_usage: Some(true),
        capture_content: Some(true),
        capture_reasoning_content: Some(true),
        capture_tool_calls: Some(true),
        ..ChatOptions::default()
    };

    if let Some(effort) = request.reasoning.as_ref().and_then(|r| r.effort.as_ref()) {
        options.reasoning_effort = Some(map_reasoning_effort(effort));
    }

    if let Some(ref tier) = request.service_tier {
        use genai::chat::ServiceTier;
        options.service_tier = Some(match tier.as_str() {
            "flex" => ServiceTier::Flex,
            "auto" => ServiceTier::Auto,
            "default" => ServiceTier::Default,
            other => {
                tracing::warn!(service_tier = %other, "Unknown service tier");
                ServiceTier::Default
            }
        });
    }

    if let Some(ref text) = request.text {
        if let Some(ref verbosity) = text.verbosity {
            options.verbosity = Some(match verbosity {
                codex_api::OpenAiVerbosity::Low => Verbosity::Low,
                codex_api::OpenAiVerbosity::Medium => Verbosity::Medium,
                codex_api::OpenAiVerbosity::High => Verbosity::High,
            });
        }
        if let Some(ref fmt) = text.format {
            options.response_format = Some(ChatResponseFormat::JsonSpec(JsonSpec {
                name: fmt.name.clone(),
                description: None,
                schema: fmt.schema.clone(),
            }));
        }
    }

    options.prompt_cache_key = request.prompt_cache_key.clone();

    // Forward tool_choice (auto/none/required/<tool-name>) and
    // parallel_tool_calls. genai has no native parallel_tool_calls field, so it
    // rides in extra_body as a top-level scalar (shallow x_merge is safe here).
    options.tool_choice = Some(map_tool_choice(&request.tool_choice));
    options.extra_body = Some(serde_json::json!({
        "parallel_tool_calls": request.parallel_tool_calls
    }));

    // Merge all headers for this request
    options.extra_headers = Some(build_extra_headers(api_provider, api_auth, &extra_headers));

    options
}

/// Maps a codex `tool_choice` string to a genai `ToolChoice`.
///
/// `"auto"` / `"none"` / `"required"` map to the matching variant; any other
/// non-empty string is treated as a specific tool name to force. genai's
/// adapter handles the Chat Completions vs Responses wire-shape difference
/// (nested `function.name` vs flat `name`) internally.
fn map_tool_choice(tool_choice: &str) -> ToolChoice {
    match tool_choice {
        "auto" => ToolChoice::Auto,
        "none" => ToolChoice::None,
        "required" => ToolChoice::Required,
        name => ToolChoice::tool(name),
    }
}

/// Maps a codex `ReasoningEffort` to a genai `ReasoningEffort`.
///
/// `XHigh` and `Max` map 1:1 (genai supports both natively). `Ultra` and
/// `Custom` have no genai equivalent and fall back (`Ultra`→`Max`,
/// `Custom`→`High`) with a warning — silently dropping a requested effort
/// level would lose intent.
fn map_reasoning_effort(
    effort: &codex_protocol::openai_models::ReasoningEffort,
) -> genai::chat::ReasoningEffort {
    use codex_protocol::openai_models::ReasoningEffort as Codex;
    use genai::chat::ReasoningEffort;
    match effort {
        Codex::None => ReasoningEffort::None,
        Codex::Minimal => ReasoningEffort::Minimal,
        Codex::Low => ReasoningEffort::Low,
        Codex::Medium => ReasoningEffort::Medium,
        Codex::High => ReasoningEffort::High,
        Codex::XHigh => ReasoningEffort::XHigh,
        Codex::Max => ReasoningEffort::Max,
        Codex::Ultra => {
            tracing::warn!("Ultra reasoning effort has no genai equivalent, mapping to Max");
            ReasoningEffort::Max
        }
        Codex::Custom(s) => {
            tracing::warn!(
                custom_effort = %s,
                "Custom reasoning effort not supported in genai, using High"
            );
            ReasoningEffort::High
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_tool_choice_maps_known_values_and_tool_names() {
        assert!(matches!(map_tool_choice("auto"), ToolChoice::Auto));
        assert!(matches!(map_tool_choice("none"), ToolChoice::None));
        assert!(matches!(map_tool_choice("required"), ToolChoice::Required));
        // Any other non-empty string forces that specific tool.
        match map_tool_choice("get_weather") {
            ToolChoice::Tool { name } => assert_eq!(name, "get_weather"),
            other => panic!("expected ToolChoice::Tool, got {other:?}"),
        }
    }

    #[test]
    fn map_reasoning_effort_preserves_xhigh_and_max() {
        use codex_protocol::openai_models::ReasoningEffort as Codex;
        use genai::chat::ReasoningEffort;
        // XHigh/Max must map 1:1 — previously downgraded to High.
        assert!(matches!(
            map_reasoning_effort(&Codex::XHigh),
            ReasoningEffort::XHigh
        ));
        assert!(matches!(map_reasoning_effort(&Codex::Max), ReasoningEffort::Max));
        // Ultra has no genai equivalent → falls back to Max (closest, not High).
        assert!(matches!(
            map_reasoning_effort(&Codex::Ultra),
            ReasoningEffort::Max
        ));
        // Lower levels map 1:1.
        assert!(matches!(
            map_reasoning_effort(&Codex::Medium),
            ReasoningEffort::Medium
        ));
    }
}
