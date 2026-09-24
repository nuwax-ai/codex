use codex_api::ResponsesApiRequest;
use codex_protocol::models::{
    ContentItem, FunctionCallOutputBody, ImageReference, ResponseItem,
};
use codex_protocol::openai_models::ReasoningEffort;
use codex_tools::code_mode_name_for_tool_name;
use rig_core::completion::message::ImageMediaType;
use rig_core::completion::message::{
    AssistantContent, DocumentSourceKind, Image as RigImage, Message, ToolResultContent,
    UserContent,
};
use rig_core::completion::request::ToolDefinition;
use rig_core::completion::CompletionRequest;
use serde_json::Value;

/// Converts a Codex `ResponsesApiRequest` into a rig `CompletionRequest`.
///
/// Returns `None` if the input contains no convertible messages (e.g. only
/// internal/local items).
pub(crate) fn responses_request_to_completion_request(
    request: &ResponsesApiRequest,
) -> Option<CompletionRequest> {
    // rig requires at least one message; the system prompt alone does not
    // count as a convertible turn for our purposes.
    let mut chat_history = convert_response_items(&request.input);
    if chat_history.is_empty() {
        return None;
    }
    if !request.instructions.is_empty() {
        chat_history.insert(
            0,
            Message::System {
                content: request.instructions.clone(),
            },
        );
    }

    // `request.tools` is `Option<ResponsesApiTools>` (opaque raw JSON). Extract
    // the tool array via the Serialize impl (as_raw_value is pub(crate)-gated).
    let mut tools: Vec<Value> = request
        .tools
        .as_ref()
        .and_then(|t| serde_json::to_value(t).ok())
        .and_then(|v| serde_json::from_value::<Vec<Value>>(v).ok())
        .unwrap_or_default();
    // Responses-Lite carries the tool list inside an `AdditionalTools` input
    // item instead of `request.tools`; merge both before flattening.
    for item in &request.input {
        if let ResponseItem::AdditionalTools { tools: extra, .. } = item {
            tools.extend(extra.iter().cloned());
        }
    }
    let tools = parse_tools(&tools);

    Some(CompletionRequest {
        model: Some(request.model.clone()),
        preamble: None,
        chat_history,
        documents: Vec::new(),
        tools,
        temperature: None,
        max_tokens: None,
        tool_choice: map_tool_choice(&request.tool_choice),
        additional_params: build_additional_params(request),
        // `text.format` → structured output. rig serializes the schema as
        // `response_format: {json_schema, strict: true}`; note rig always
        // requests strict validation, so codex's `strict: false` cannot be
        // honored (documented in the field-mapping audit).
        output_schema: request.text.as_ref().and_then(|text| {
            text.format
                .as_ref()
                .and_then(|format| serde_json::from_value(format.schema.clone()).ok())
        }),
        record_telemetry_content: false,
    })
}

/// Passes through provider-specific request knobs that rig's chat wire does
/// not type natively. `reasoning_effort` needs no clamping here — rig
/// forwards the raw JSON value, so XHigh/Max/Ultra/Persistent reach the
/// provider verbatim.
fn build_additional_params(request: &ResponsesApiRequest) -> Option<Value> {
    let mut params = serde_json::Map::new();
    // `text.verbosity` has no typed field on rig's chat wire; forward
    // verbatim (same string the OpenAI Chat API accepts).
    if let Some(verbosity) = request
        .text
        .as_ref()
        .and_then(|text| text.verbosity.as_ref())
    {
        params.insert(
            "verbosity".into(),
            Value::String(
                match verbosity {
                    codex_api::OpenAiVerbosity::Low => "low",
                    codex_api::OpenAiVerbosity::Medium => "medium",
                    codex_api::OpenAiVerbosity::High => "high",
                }
                .to_string(),
            ),
        );
    }
    if let Some(reasoning) = request.reasoning.as_ref()
        && let Some(effort) = reasoning.effort.as_ref()
        && *effort != ReasoningEffort::Medium
    {
        params.insert(
            "reasoning_effort".into(),
            Value::String(effort.as_str().to_string()),
        );
    }
    if let Some(tier) = request.service_tier.as_ref() {
        params.insert("service_tier".into(), Value::String(tier.clone()));
    }
    if !request.parallel_tool_calls {
        params.insert("parallel_tool_calls".into(), Value::Bool(false));
    }
    if let Some(key) = request.prompt_cache_key.as_ref() {
        params.insert("prompt_cache_key".into(), Value::String(key.clone()));
    }
    if params.is_empty() {
        None
    } else {
        Some(Value::Object(params))
    }
}

fn map_tool_choice(choice: &str) -> Option<rig_core::completion::message::ToolChoice> {
    use rig_core::completion::message::ToolChoice;
    match choice {
        "auto" => Some(ToolChoice::Auto),
        "none" => Some(ToolChoice::None),
        "required" => Some(ToolChoice::Required),
        // codex only sends flat strings on this path; a specific-function
        // choice would arrive as `{"type":"function","name":...}` which the
        // Responses layer keeps structured, so a bare name string here is
        // treated as a specific-function pin.
        other => Some(ToolChoice::Specific {
            function_names: vec![other.to_string()],
        }),
    }
}

fn convert_response_items(items: &[ResponseItem]) -> Vec<Message> {
    let mut messages: Vec<Message> = Vec::new();

    // rig's ToolResult requires the executed tool's *name*, which codex only
    // carries on the FunctionCall item — index call_id → name up front.
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for item in items {
        if let ResponseItem::FunctionCall { name, call_id, .. } = item {
            tool_names.insert(call_id.clone(), name.clone());
        }
    }

    for item in items {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let role = role.as_str();
                if role == "assistant" {
                    let parts = convert_assistant_content(content);
                    if !parts.is_empty() {
                        // Consecutive assistant items belong to the same
                        // model turn (history order: Reasoning → Message →
                        // FunctionCall) and MUST merge into one assistant
                        // message — a reasoning-only message ahead of the
                        // text is dropped by the OpenAI wire, losing the
                        // DeepSeek-required reasoning replay.
                        match messages.last_mut() {
                            Some(Message::Assistant { content, .. }) => {
                                content.extend(parts);
                            }
                            _ => messages.push(Message::Assistant {
                                id: None,
                                content: parts,
                            }),
                        }
                    }
                } else {
                    // "user", "developer"/"system" and anything unknown map to
                    // a user turn — Chat Completions only knows two roles.
                    let parts = convert_user_content(content);
                    if !parts.is_empty() {
                        messages.push(Message::User { content: parts });
                    }
                }
            }
            ResponseItem::Reasoning {
                encrypted_content, ..
            } => {
                if let Some(ec) = encrypted_content {
                    // Echo the reasoning back on the next request — DeepSeek
                    // thinking mode requires replaying reasoning content.
                    // rig's reasoning provenance marks hand-built reasoning as
                    // provider-unknown, which replays verbatim.
                    match messages.last_mut() {
                        Some(Message::Assistant { content, .. }) => {
                            content.push(AssistantContent::reasoning(ec));
                        }
                        _ => messages.push(Message::Assistant {
                            id: None,
                            content: vec![AssistantContent::reasoning(ec)],
                        }),
                    }
                }
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                ..
            } => {
                let args = serde_json::from_str::<Value>(arguments)
                    .unwrap_or_else(|_| Value::Object(Default::default()));
                let part = AssistantContent::tool_call(call_id.clone(), name.clone(), args);
                match messages.last_mut() {
                    Some(Message::Assistant { content, .. }) => content.push(part),
                    _ => messages.push(Message::Assistant {
                        id: None,
                        content: vec![part],
                    }),
                }
            }
            ResponseItem::FunctionCallOutput { call_id, output, .. } => {
                let content = match &output.body {
                    FunctionCallOutputBody::Text(text) => {
                        vec![ToolResultContent::text(truncate_tool_text(text))]
                    }
                    // Item-by-item conversion: images become real image
                    // blocks (data URLs decoded to base64 sources) instead of
                    // megabytes of base64 inside a text string, and text gets
                    // a defensive cap.
                    FunctionCallOutputBody::ContentItems(items) => {
                        convert_tool_output_items(items)
                    }
                };
                let call_id = call_id.clone().unwrap_or_default();
                let name = tool_names
                    .get(&call_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown_tool".to_string());
                messages.push(Message::User {
                    content: vec![UserContent::ToolResult(
                        rig_core::completion::message::ToolResult {
                            call: rig_core::completion::message::ToolCallId::new_or_mint(
                                call_id.clone(),
                            ),
                            provider: rig_core::completion::message::ProviderCallId::new(&call_id),
                            name,
                            content,
                        },
                    )],
                });
            }
            ResponseItem::CustomToolCall {
                name,
                input,
                call_id,
                ..
            } => {
                let args = serde_json::from_str::<Value>(input)
                    .unwrap_or_else(|_| Value::String(input.clone()));
                let part = AssistantContent::tool_call(call_id.clone(), name.clone(), args);
                match messages.last_mut() {
                    Some(Message::Assistant { content, .. }) => content.push(part),
                    _ => messages.push(Message::Assistant {
                        id: None,
                        content: vec![part],
                    }),
                }
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                let content = match &output.body {
                    FunctionCallOutputBody::Text(text) => text.clone(),
                    FunctionCallOutputBody::ContentItems(items) => {
                        serde_json::to_string(items).unwrap_or_default()
                    }
                };
                let call_id = call_id.clone();
                let name = tool_names
                    .get(&call_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown_tool".to_string());
                messages.push(Message::User {
                    content: vec![UserContent::ToolResult(
                        rig_core::completion::message::ToolResult {
                            call: rig_core::completion::message::ToolCallId::new_or_mint(
                                call_id.clone(),
                            ),
                            provider: rig_core::completion::message::ProviderCallId::new(&call_id),
                            name,
                            content: vec![ToolResultContent::text(content)],
                        },
                    )],
                });
            }
            // Internal/local events — not sent to the model.
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ConfigurationUpdate { .. } => {
                // Skip — these items are internal to Codex.
            }
            ResponseItem::AgentMessage { content, .. } => {
                // Multi-agent task/results traffic (NEW_TASK, MESSAGE,
                // FINAL_ANSWER) is model-visible history: forward the
                // plaintext parts as assistant text (merging with a
                // preceding assistant turn), skip provider-encrypted parts.
                let mut text = String::new();
                for part in content {
                    match part {
                        codex_protocol::models::AgentMessageInputContent::InputText {
                            text: t,
                        } => {
                            if !text.is_empty() {
                                text.push('\n');
                            }
                            text.push_str(t);
                        }
                        codex_protocol::models::AgentMessageInputContent::EncryptedContent {
                            ..
                        } => {
                            tracing::debug!(
                                "Skipping encrypted agent-message part (not replayable cross-provider)"
                            );
                        }
                    }
                }
                if !text.is_empty() {
                    match messages.last_mut() {
                        Some(Message::Assistant { content, .. }) => {
                            content.push(AssistantContent::text(text));
                        }
                        _ => messages.push(Message::Assistant {
                            id: None,
                            content: vec![AssistantContent::text(text)],
                        }),
                    }
                }
            }
            ResponseItem::Other => {
                tracing::warn!("Skipping unknown ResponseItem::Other in request conversion");
            }
        }
    }

    messages
}

/// Defensive cap on a single tool-result text part. Codex core normally
/// truncates outputs before they reach the bridge; this guards the bridge
/// against runaway payloads when that path is bypassed.
fn truncate_tool_text(text: &str) -> String {
    const MAX_TOOL_TEXT_CHARS: usize = 200_000;
    if text.len() <= MAX_TOOL_TEXT_CHARS {
        return text.to_string();
    }
    tracing::warn!(
        chars = text.len(),
        limit = MAX_TOOL_TEXT_CHARS,
        "truncating oversized tool-result text"
    );
    let mut cut = text.char_indices().nth(MAX_TOOL_TEXT_CHARS).map(|(i, _)| i).unwrap_or(text.len());
    cut = cut.min(text.len());
    format!("{}\n…[truncated]", &text[..cut])
}

/// Converts a tool-result ContentItems list into rig tool-result parts:
/// text stays text; images become typed image blocks with data URLs
/// decoded into base64 sources (never inlined into the text).
fn convert_tool_output_items(items: &[codex_protocol::models::FunctionCallOutputContentItem]) -> Vec<ToolResultContent> {
    use codex_protocol::models::FunctionCallOutputContentItem;
    let mut parts = Vec::new();
    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                parts.push(ToolResultContent::text(truncate_tool_text(text)));
            }
            FunctionCallOutputContentItem::InputImage { image, .. } => {
                match image {
                    ImageReference::Inline { image_url } => {
                        if let Some(part) = data_url_image_part(image_url) {
                            parts.push(part);
                        } else {
                            // A plain network URL — forward as a URL source.
                            parts.push(ToolResultContent::Image(RigImage {
                                data: DocumentSourceKind::Url(image_url.clone()),
                                media_type: None,
                                detail: None,
                                additional_params: None,
                            }));
                        }
                    }
                    ImageReference::File { file_id } => {
                        tracing::warn!(
                            file_id,
                            "Skipping file-referenced image in tool result (unsupported)"
                        );
                    }
                }
            }
            FunctionCallOutputContentItem::InputAudio { .. } => {
                tracing::warn!("Skipping audio in tool result (unsupported by bridge)");
            }
            FunctionCallOutputContentItem::EncryptedContent { .. } => {
                // Provider-encrypted content cannot be replayed through a
                // different provider; a placeholder keeps the tool result
                // visible to the model without leaking opaque bytes.
                parts.push(ToolResultContent::text("(encrypted content omitted)"));
            }
        }
    }
    if parts.is_empty() {
        parts.push(ToolResultContent::text("(empty tool result)"));
    }
    parts
}

/// Parses a `data:<mime>;base64,<payload>` URL into a base64-backed image.
/// The Anthropic wire renders URL sources as remote links — inline payloads
/// MUST be base64 sources or the content is lost.
fn data_url_image(data_url: &str) -> Option<RigImage> {
    let rest = data_url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.strip_suffix(";base64")?;
    let media_type = match mime {
        "image/jpeg" | "image/jpg" => ImageMediaType::JPEG,
        "image/png" => ImageMediaType::PNG,
        "image/gif" => ImageMediaType::GIF,
        "image/webp" => ImageMediaType::WEBP,
        "image/heic" => ImageMediaType::HEIC,
        "image/heif" => ImageMediaType::HEIF,
        "image/svg+xml" => ImageMediaType::SVG,
        other => {
            tracing::warn!(mime = other, "Unsupported image mime in tool result, skipping");
            return None;
        }
    };
    Some(RigImage {
        data: DocumentSourceKind::Base64(payload.to_string()),
        media_type: Some(media_type),
        detail: None,
        additional_params: None,
    })
}

fn data_url_image_part(data_url: &str) -> Option<ToolResultContent> {
    Some(ToolResultContent::Image(data_url_image(data_url)?))
}

fn convert_user_content(items: &[ContentItem]) -> Vec<UserContent> {
    items
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(UserContent::text(text.clone()))
            }
            ContentItem::InputImage { image, .. } => {
                let ImageReference::Inline { image_url } = image else {
                    tracing::warn!(
                        "Skipping file-referenced image (unsupported by rig bridge)"
                    );
                    return None;
                };
                // data: URLs decode into base64 sources (remote-link
                // rendering on the Anthropic wire would lose the content);
                // plain http(s) URLs forward as URL sources.
                Some(UserContent::Image(
                    data_url_image(image_url).unwrap_or(RigImage {
                        data: DocumentSourceKind::Url(image_url.clone()),
                        media_type: None,
                        detail: None,
                        additional_params: None,
                    }),
                ))
            }
            ContentItem::InputAudio { .. } => {
                // 国内 LLM 适配暂不支持音频输入，跳过。
                tracing::warn!("Skipping ContentItem::InputAudio (unsupported by rig bridge)");
                None
            }
        })
        .collect()
}

fn convert_assistant_content(items: &[ContentItem]) -> Vec<AssistantContent> {
    items
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(AssistantContent::text(text.clone()))
            }
            ContentItem::InputImage { image, .. } => {
                let ImageReference::Inline { image_url } = image else {
                    return None;
                };
                Some(AssistantContent::Image(
                    data_url_image(image_url).unwrap_or(RigImage {
                        data: DocumentSourceKind::Url(image_url.clone()),
                        media_type: None,
                        detail: None,
                        additional_params: None,
                    }),
                ))
            }
            ContentItem::InputAudio { .. } => None,
        })
        .collect()
}

/// Parses the Responses-API tool specs into flat rig `ToolDefinition`s.
/// Namespace tools (`{"type":"namespace", ...}`) are flattened into
/// `mcp__<server>__<tool>` function names — the same convention the
/// registry's flat-name index resolves on the way back.
fn parse_tools(tools: &[Value]) -> Vec<ToolDefinition> {
    let mut parsed = Vec::new();
    for v in tools {
        match v.get("type").and_then(|t| t.as_str()) {
            Some("function") | Some("custom") => {
                if let Some(def) = flat_function_tool(v) {
                    parsed.push(def);
                }
            }
            Some("namespace") => {
                let Some(children) = v.get("tools").and_then(|t| t.as_array()) else {
                    continue;
                };
                let namespace = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("namespace");
                for child in children {
                    let Some(def) = flat_function_tool(child) else {
                        continue;
                    };
                    let flat =
                        code_mode_name_for_tool_name(&codex_tools::ToolName::namespaced(
                            namespace,
                            def.name.as_str(),
                        ));
                    parsed.push(ToolDefinition {
                        name: flat,
                        description: def.description,
                        parameters: def.parameters,
                    });
                }
            }
            // Responses-only hosted tools (web_search etc.) have no Chat
            // Completions equivalent; drop with a warning like LiteLLM does.
            // warn-level because losing a tool is a silent capability
            // regression the user should be able to see in logs.
            Some(other) => {
                tracing::warn!(tool_type = other, "Dropping non-function tool (no Chat Completions equivalent)");
            }
            None => {}
        }
    }
    parsed
}

fn flat_function_tool(v: &Value) -> Option<ToolDefinition> {
    let name = v.get("name").and_then(|n| n.as_str())?;
    Some(ToolDefinition {
        name: name.to_string(),
        description: v
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .to_string(),
        parameters: v.get("parameters").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request(input: Vec<ResponseItem>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "test-model".into(),
            instructions: "be helpful".into(),
            input,
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
            access_programs: None,
        }
    }

    fn user_text(text: &str) -> ResponseItem {
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

    fn assistant_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".into(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn reasoning_item(content: &str) -> ResponseItem {
        ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: Some(vec![codex_protocol::models::ReasoningItemContent::ReasoningText {
                text: content.to_string(),
            }]),
            encrypted_content: Some(content.to_string()),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn function_call(call_id: &str, name: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.into(),
            namespace: None,
            arguments: arguments.into(),
            encrypted_function_args: None,
            call_id: call_id.into(),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn tool_output(call_id: &str, output: &str) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(call_id.into()),
            name: None,
            namespace: None,
            output: codex_protocol::models::FunctionCallOutputPayload {
                body: FunctionCallOutputBody::Text(output.into()),
                success: Some(true),
            },
            internal_chat_message_metadata_passthrough: None,
        }
    }

    #[test]
    fn empty_input_yields_none() {
        assert!(responses_request_to_completion_request(&base_request(vec![])).is_none());
    }

    #[test]
    fn system_prompt_becomes_leading_system_message() {
        let req = responses_request_to_completion_request(&base_request(vec![user_text("hi")]))
            .expect("convertible");
        assert_eq!(req.chat_history.len(), 2);
        assert!(matches!(
            req.chat_history.first(),
            Some(Message::System { content }) if content == "be helpful"
        ));
    }

    #[test]
    fn reasoning_echoes_into_previous_assistant_message() {
        let req = responses_request_to_completion_request(&base_request(vec![
            assistant_text("answer"),
            reasoning_item("thinking..."),
        ]))
        .expect("convertible");
        // system + one assistant message carrying text AND reasoning
        assert_eq!(req.chat_history.len(), 2);
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant") else {
            panic!("expected assistant message");
        };
        assert_eq!(content.len(), 2);
        assert!(matches!(content[0], AssistantContent::Text(_)));
        assert!(matches!(content[1], AssistantContent::Reasoning(_)));
    }

    #[test]
    fn function_call_merges_into_last_assistant_message() {
        let req = responses_request_to_completion_request(&base_request(vec![
            user_text("weather?"),
            assistant_text("let me check"),
            function_call("call_1", "get_weather", r#"{"city":"北京"}"#),
        ]))
        .expect("convertible");
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant")
        else {
            panic!("expected assistant message");
        };
        // text + tool call in one assistant message (Chat Completions shape)
        assert_eq!(content.len(), 2);
        assert!(matches!(content[1], AssistantContent::ToolCall(_)));
    }

    #[test]
    fn tool_result_resolves_name_from_call_id_and_lands_in_user_message() {
        let req = responses_request_to_completion_request(&base_request(vec![
            function_call("call_1", "get_weather", r#"{"city":"北京"}"#),
            tool_output("call_1", "sunny"),
        ]))
        .expect("convertible");
        let Message::User { content } = req.chat_history.last().expect("user") else {
            panic!("expected user message");
        };
        let UserContent::ToolResult(result) = content.last().expect("tool result") else {
            panic!("expected tool result");
        };
        assert_eq!(result.name, "get_weather");
        assert!(matches!(
            result.content.first(),
            Some(ToolResultContent::Text(_))
        ));
    }

    #[test]
    fn namespace_tools_flatten_to_mcp_names() {
        let tools_json = r#"[{
            "type": "namespace",
            "name": "mcp__memory",
            "tools": [
                {"type": "function", "name": "create_entities", "description": "d",
                 "parameters": {"type": "object"}}
            ]
        }]"#;
        let tools: Vec<Value> = serde_json::from_str(tools_json).expect("json");
        let parsed = parse_tools(&tools);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "mcp__memory__create_entities");
    }

    #[test]
    fn hosted_tools_are_dropped() {
        let tools_json = r#"[
            {"type": "web_search"},
            {"type": "function", "name": "f", "description": "", "parameters": {}}
        ]"#;
        let tools: Vec<Value> = serde_json::from_str(tools_json).expect("json");
        let parsed = parse_tools(&tools);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "f");
    }

    #[test]
    fn tool_choice_maps_known_values() {
        use rig_core::completion::message::ToolChoice;
        assert!(matches!(map_tool_choice("auto"), Some(ToolChoice::Auto)));
        assert!(matches!(map_tool_choice("none"), Some(ToolChoice::None)));
        assert!(matches!(map_tool_choice("required"), Some(ToolChoice::Required)));
        assert!(matches!(
            map_tool_choice("get_weather"),
            Some(ToolChoice::Specific { .. })
        ));
    }

    #[test]
    fn high_reasoning_efforts_pass_through_unclamped() {
        let mut request = base_request(vec![user_text("hi")]);
        request.reasoning = Some(codex_api::Reasoning {
            effort: Some(ReasoningEffort::XHigh),
            summary: None,
            context: None,
        });
        let req =
            responses_request_to_completion_request(&request).expect("convertible");
        let params = req.additional_params.expect("params present");
        assert_eq!(params["reasoning_effort"], "xhigh");
    }
}

#[cfg(test)]
mod text_format_tests {
    use super::*;

    #[test]
    fn text_format_maps_to_output_schema() {
        let mut request = base_request(vec![user_text_fixture("hi")]);
        request.text = Some(codex_api::TextControls {
            verbosity: None,
            format: Some(codex_api::TextFormat {
                r#type: codex_api::TextFormatType::JsonSchema,
                strict: false,
                schema: serde_json::json!({"type": "object", "properties": {"answer": {"type": "string"}}}),
                name: "answer_shape".into(),
            }),
        });
        let req = responses_request_to_completion_request(&request).expect("convertible");
        let schema = req.output_schema.expect("output_schema mapped");
        assert!(schema.to_value().get("properties").is_some());
    }

    #[test]
    fn verbosity_maps_to_additional_params() {
        let mut request = base_request(vec![user_text_fixture("hi")]);
        request.text = Some(codex_api::TextControls {
            verbosity: Some(codex_api::OpenAiVerbosity::Low),
            format: None,
        });
        let req = responses_request_to_completion_request(&request).expect("convertible");
        assert_eq!(req.additional_params.expect("params")["verbosity"], "low");
    }

    fn base_request(input: Vec<ResponseItem>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "m".into(),
            instructions: String::new(),
            input,
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
            access_programs: None,
        }
    }

    fn user_text_fixture(text: &str) -> ResponseItem {
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
}

#[cfg(test)]
mod review_fix_tests {
    use super::*;

    fn base_request(input: Vec<ResponseItem>) -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "m".into(),
            instructions: String::new(),
            input,
            tools: None,
            tool_choice: "auto".into(),
            parallel_tool_calls: true,
            reasoning: None,
            store: false,
            stream: true,
            stream_options: None,
            include: vec![],
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
            access_programs: None,
        }
    }

    fn user_text(text: &str) -> ResponseItem {
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

    fn assistant_text(text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: "assistant".into(),
            content: vec![ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn reasoning(content: &str) -> ResponseItem {
        ResponseItem::Reasoning {
            id: None,
            summary: vec![],
            content: Some(vec![codex_protocol::models::ReasoningItemContent::ReasoningText {
                text: content.to_string(),
            }]),
            encrypted_content: Some(content.to_string()),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    fn function_call(call_id: &str, name: &str, arguments: &str) -> ResponseItem {
        ResponseItem::FunctionCall {
            id: None,
            name: name.into(),
            namespace: None,
            arguments: arguments.into(),
            encrypted_function_args: None,
            call_id: call_id.into(),
            internal_chat_message_metadata_passthrough: None,
        }
    }

    /// Real history order is Reasoning → Message → FunctionCall; all three
    /// MUST merge into a single assistant message so the reasoning replay
    /// survives the OpenAI wire (a reasoning-only assistant message ahead
    /// of the text gets dropped).
    #[test]
    fn real_history_order_merges_into_one_assistant_message() {
        let req = responses_request_to_completion_request(&base_request(vec![
            user_text("weather?"),
            reasoning("thinking about it"),
            assistant_text("let me check"),
            function_call("c1", "get_weather", r#"{"city":"北京"}"#),
        ]))
        .expect("convertible");
        let assistant_count = req
            .chat_history
            .iter()
            .filter(|m| matches!(m, Message::Assistant { .. }))
            .count();
        assert_eq!(
            assistant_count, 1,
            "reasoning+text+tool call must be ONE assistant message"
        );
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant")
        else {
            panic!()
        };
        // reasoning + text + tool call, in order
        assert!(matches!(content[0], AssistantContent::Reasoning(_)));
        assert!(matches!(content[1], AssistantContent::Text(_)));
        assert!(matches!(content[2], AssistantContent::ToolCall(_)));
    }

    /// AgentMessage plaintext reaches the model as assistant text.
    #[test]
    fn agent_message_forwards_plaintext() {
        let req = responses_request_to_completion_request(&base_request(vec![
            ResponseItem::AgentMessage {
                id: None,
                author: "agent".into(),
                recipient: "main".into(),
                content: vec![
                    codex_protocol::models::AgentMessageInputContent::InputText {
                        text: "task complete".into(),
                    },
                    codex_protocol::models::AgentMessageInputContent::EncryptedContent {
                        encrypted_content: "opaque".into(),
                    },
                ],
                internal_chat_message_metadata_passthrough: None,
            },
        ]))
        .expect("convertible");
        let Message::Assistant { content, .. } = req.chat_history.last().expect("assistant")
        else {
            panic!("expected assistant message");
        };
        assert!(content.iter().any(|part| matches!(
            part,
            AssistantContent::Text(t) if t.text.contains("task complete")
        )));
    }

    /// Tool-result images become typed image blocks, never base64 text.
    #[test]
    fn tool_result_data_url_image_becomes_image_block() {
        let req = responses_request_to_completion_request(&base_request(vec![
            function_call("c1", "view_image", "{}"),
            ResponseItem::FunctionCallOutput {
                id: None,
                call_id: Some("c1".into()),
                name: None,
                namespace: None,
                output: codex_protocol::models::FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::ContentItems(vec![
                        codex_protocol::models::FunctionCallOutputContentItem::InputImage {
                            image: ImageReference::Inline {
                                image_url: "data:image/png;base64,aGVsbG8=".into(),
                            },
                            detail: None,
                        },
                        codex_protocol::models::FunctionCallOutputContentItem::InputText {
                            text: "screenshot".into(),
                        },
                    ]),
                    success: Some(true),
                },
                internal_chat_message_metadata_passthrough: None,
            },
        ]))
        .expect("convertible");
        let Message::User { content } = req.chat_history.last().expect("user") else {
            panic!()
        };
        let UserContent::ToolResult(result) = content.last().expect("tool result") else {
            panic!()
        };
        assert_eq!(result.content.len(), 2);
        assert!(matches!(
            result.content[0],
            ToolResultContent::Image(ref img)
                if matches!(img.data, DocumentSourceKind::Base64(_))
        ));
        // No part carries raw base64 inside text.
        for part in &result.content {
            if let ToolResultContent::Text(t) = part {
                assert!(!t.text.contains("aGVsbG8="), "base64 leaked into text");
            }
        }
    }

    /// Input data-URL images decode into base64 sources (Anthropic wire
    /// would drop them as remote links).
    #[test]
    fn input_data_url_image_decodes_to_base64() {
        let req = responses_request_to_completion_request(&base_request(vec![ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![ContentItem::InputImage {
                image: ImageReference::Inline {
                    image_url: "data:image/jpeg;base64,QUJD".into(),
                },
                detail: None,
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }]))
        .expect("convertible");
        let Message::User { content } = req.chat_history.last().expect("user") else {
            panic!()
        };
        assert!(matches!(
            content[0],
            UserContent::Image(ref img)
                if matches!(img.data, DocumentSourceKind::Base64(_))
        ));
    }
}
