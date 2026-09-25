//! History conversion shared by the two Rig wire protocols.
use crate::client::RigProtocol;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseItem;
use rig_core::completion::message::AssistantContent;
use rig_core::completion::message::DocumentSourceKind;
use rig_core::completion::message::Image as RigImage;
use rig_core::completion::message::ImageMediaType;
use rig_core::completion::message::Message;
use rig_core::completion::message::ToolResultContent;
use rig_core::completion::message::UserContent;
use serde_json::Value;

pub(crate) fn convert_response_items(
    items: &[ResponseItem],
    protocol: RigProtocol,
    source: &str,
) -> Result<Vec<Message>, codex_api::ApiError> {
    let mut messages: Vec<Message> = Vec::new();

    // rig's ToolResult requires the executed tool's *name*, which codex only
    // carries on the FunctionCall item — index call_id → name up front.
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for item in items {
        if let ResponseItem::FunctionCall {
            name,
            call_id,
            namespace,
            ..
        }
        | ResponseItem::CustomToolCall {
            name,
            call_id,
            namespace,
            ..
        } = item
        {
            tool_names.insert(
                call_id.clone(),
                crate::request_tools::flat_name(name, namespace.as_deref()),
            );
        }
    }

    // Tool calls whose historical record is unreplayable (e.g. unparseable
    // arguments). Their outputs must be skipped too — a tool result without
    // its call is a guaranteed provider 400 on both wires.
    let mut dropped_calls: std::collections::HashSet<String> = Default::default();

    for item in items {
        match item {
            ResponseItem::Message { role, content, .. } => {
                let role = role.as_str();
                if role == "assistant" {
                    let parts = convert_assistant_content(content, protocol);
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
                } else if matches!(role, "system" | "developer") {
                    let mut text = Vec::new();
                    for part in content {
                        match part {
                            ContentItem::InputText { text: part } | ContentItem::OutputText { text: part } => text.push(part.as_str()),
                            _ => return Err(codex_api::ApiError::InvalidRequest { message: "Non-text system/developer content cannot be sent through the Rig bridge".into() }),
                        }
                    }
                    messages.push(Message::System {
                        content: text.join("\n"),
                    });
                } else if role == "user" {
                    let parts = convert_user_content(content, protocol);
                    if !parts.is_empty() {
                        messages.push(Message::User { content: parts });
                    }
                } else {
                    return Err(codex_api::ApiError::InvalidRequest {
                        message: format!("Unsupported message role: {role}"),
                    });
                }
            }
            ResponseItem::Reasoning { .. } => {
                let mut blocks = crate::reasoning::replay_reasoning(item, source, protocol);
                if protocol == RigProtocol::Anthropic && blocks.len() > 1 {
                    // Anthropic allows at most one thinking block per
                    // assistant message; extra blocks are a guaranteed 400.
                    tracing::warn!(
                        blocks = blocks.len(),
                        "Replaying only the first reasoning block on the Anthropic wire"
                    );
                    blocks.truncate(1);
                }
                for reasoning in blocks {
                    match messages.last_mut() {
                        Some(Message::Assistant { content, .. }) => {
                            content.push(AssistantContent::Reasoning(reasoning));
                        }
                        _ => messages.push(Message::Assistant {
                            id: None,
                            content: vec![AssistantContent::Reasoning(reasoning)],
                        }),
                    }
                }
            }
            ResponseItem::FunctionCall {
                name,
                arguments,
                call_id,
                namespace,
                ..
            } => {
                // Historical arguments are recorded data, not fresh input: a
                // single unparseable entry (imported sessions, older bridge
                // output) must not brick every subsequent request. Skip the
                // call — and its matching output below — with a warning.
                let args = if arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    match serde_json::from_str::<Value>(arguments) {
                        Ok(args) => args,
                        Err(error) => {
                            tracing::warn!(
                                tool = %name,
                                call_id = %call_id,
                                %error,
                                "Skipping historical tool call with unparseable arguments"
                            );
                            dropped_calls.insert(call_id.clone());
                            continue;
                        }
                    }
                };
                let part = AssistantContent::tool_call(
                    call_id.clone(),
                    crate::request_tools::flat_name(name, namespace.as_deref()),
                    args,
                );
                match messages.last_mut() {
                    Some(Message::Assistant { content, .. }) => content.push(part),
                    _ => messages.push(Message::Assistant {
                        id: None,
                        content: vec![part],
                    }),
                }
            }
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } => {
                if dropped_calls.contains(call_id.as_deref().unwrap_or_default()) {
                    tracing::warn!(
                        call_id = ?call_id,
                        "Skipping tool output whose call was dropped from replay"
                    );
                    continue;
                }
                append_tool_result(
                    &mut messages,
                    call_id.as_deref().unwrap_or_default(),
                    output,
                    &tool_names,
                    protocol,
                );
            }
            ResponseItem::CustomToolCall {
                name,
                input,
                call_id,
                namespace,
                ..
            } => {
                // Custom tools are declared with an {"input": string}
                // wrapper schema, so history replays use the same shape the
                // model was told to produce.
                let args = serde_json::json!({ "input": input });
                let part = AssistantContent::tool_call(
                    call_id.clone(),
                    crate::request_tools::flat_name(name, namespace.as_deref()),
                    args,
                );
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
                append_tool_result(&mut messages, call_id, output, &tool_names, protocol);
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
                        codex_protocol::models::AgentMessageInputContent::InputText { text: t } => {
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

    Ok(messages)
}

fn append_tool_result(
    messages: &mut Vec<Message>,
    call_id: &str,
    output: &codex_protocol::models::FunctionCallOutputPayload,
    tool_names: &std::collections::HashMap<String, String>,
    protocol: RigProtocol,
) {
    let parts = match &output.body {
        FunctionCallOutputBody::Text(text) => {
            vec![ToolResultContent::text(truncate_tool_text(text))]
        }
        FunctionCallOutputBody::ContentItems(items) => convert_tool_output_items(items, protocol),
    };
    let mut content = Vec::new();
    let mut images = Vec::new();
    for part in parts {
        match part {
            ToolResultContent::Image(image)
                if protocol == RigProtocol::Chat
                    || matches!(image.data, DocumentSourceKind::Url(_)) =>
            {
                images.push(UserContent::Image(image));
            }
            part => content.push(part),
        }
    }
    if !images.is_empty() {
        content.push(ToolResultContent::text(
            "Images from this tool result are attached in the following user message.",
        ));
    }
    let result = UserContent::ToolResult(rig_core::completion::message::ToolResult {
        call: rig_core::completion::message::ToolCallId::new_or_mint(call_id.to_string()),
        provider: rig_core::completion::message::ProviderCallId::new(call_id),
        name: tool_names
            .get(call_id)
            .cloned()
            .unwrap_or_else(|| "unknown_tool".into()),
        content,
    });
    // Accumulate adjacent tool results before supplemental images. Rig's Chat serializer
    // emits ToolResult entries first, preserving the required parallel call/result order.
    match messages.last_mut() {
        Some(Message::User { content })
            if content
                .iter()
                .any(|part| matches!(part, UserContent::ToolResult(_))) =>
        {
            content.insert(
                content
                    .iter()
                    .take_while(|part| matches!(part, UserContent::ToolResult(_)))
                    .count(),
                result,
            )
        }
        _ => messages.push(Message::User {
            content: vec![result],
        }),
    }
    if !images.is_empty()
        && let Some(Message::User { content }) = messages.last_mut()
    {
        content.push(UserContent::text(format!(
            "Images returned by tool call {call_id}:"
        )));
        content.extend(images);
    }
}

/// Defensive cap on a single tool-result text part. Codex core truncates
/// outputs at its own budget before they reach the bridge (default 10,000
/// tokens, see core's `DEFAULT_MAX_OUTPUT_TOKENS`); this safety net sits well
/// above it so it only binds when that path is bypassed (e.g. runaway base64
/// payloads), never on legitimate tool outputs.
fn truncate_tool_text(text: &str) -> String {
    codex_utils_string::truncate_middle_with_token_budget(text, 24_000).0
}

/// Converts a tool-result ContentItems list into rig tool-result parts:
/// text stays text; images become typed image blocks with data URLs
/// decoded into base64 sources (never inlined into the text).
fn convert_tool_output_items(
    items: &[codex_protocol::models::FunctionCallOutputContentItem],
    protocol: RigProtocol,
) -> Vec<ToolResultContent> {
    use codex_protocol::models::FunctionCallOutputContentItem;
    let mut parts = Vec::new();
    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                parts.push(ToolResultContent::text(truncate_tool_text(text)));
            }
            FunctionCallOutputContentItem::InputImage { image, detail } => match image {
                ImageReference::Inline { image_url } => {
                    if let Some(image) = image_from_url(image_url, *detail, protocol) {
                        parts.push(ToolResultContent::Image(image));
                    }
                }
                ImageReference::File { file_id } => {
                    tracing::warn!(
                        file_id,
                        "Skipping file-referenced image in tool result (unsupported)"
                    );
                }
            },
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
/// Returns `None` for any data URL the bridge cannot decode (unsupported
/// mime, missing base64 marker); the protocol-aware decision of what to do
/// with it belongs to the caller. The Anthropic wire renders URL sources as
/// remote links — inline payloads MUST be base64 sources there.
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
        _ => return None,
    };
    Some(RigImage {
        data: DocumentSourceKind::Base64(payload.to_string()),
        media_type: Some(media_type),
        detail: None,
        additional_params: None,
    })
}

fn image_from_url(
    url: &str,
    detail: Option<codex_protocol::models::ImageDetail>,
    protocol: RigProtocol,
) -> Option<RigImage> {
    use codex_protocol::models::ImageDetail as CodexDetail;
    use rig_core::completion::message::ImageDetail as RigDetail;
    let parsed = if url.starts_with("data:") {
        match data_url_image(url) {
            Some(image) => Some(image),
            None => {
                if protocol == RigProtocol::Anthropic {
                    // Anthropic URL sources must be http(s); an inline data
                    // URL there is a guaranteed 400 — drop the image.
                    tracing::warn!(
                        "Dropping undecodable data-URL image (Anthropic URL sources must be http(s))"
                    );
                    return None;
                }
                // Chat's image_url legally carries data: URLs; forward the
                // raw payload and let the provider decide.
                tracing::warn!("Forwarding undecodable data-URL image as a URL source (chat wire)");
                None
            }
        }
    } else {
        None
    };
    let mut image = parsed.unwrap_or(RigImage {
        data: DocumentSourceKind::Url(url.to_string()),
        media_type: None,
        detail: None,
        additional_params: None,
    });
    image.detail = detail.map(|detail| match detail {
        CodexDetail::Auto => RigDetail::Auto,
        CodexDetail::Low => RigDetail::Low,
        CodexDetail::High => RigDetail::High,
        CodexDetail::Original => {
            tracing::warn!("Chat image detail original is unsupported; using high");
            RigDetail::High
        }
    });
    Some(image)
}

fn convert_user_content(items: &[ContentItem], protocol: RigProtocol) -> Vec<UserContent> {
    items
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(UserContent::text(text.clone()))
            }
            ContentItem::InputImage { image, detail } => {
                let ImageReference::Inline { image_url } = image else {
                    tracing::warn!("Skipping file-referenced image (unsupported by rig bridge)");
                    return None;
                };
                // data: URLs decode into base64 sources (remote-link
                // rendering on the Anthropic wire would lose the content);
                // plain http(s) URLs forward as URL sources.
                image_from_url(image_url, *detail, protocol).map(UserContent::Image)
            }
            ContentItem::InputAudio { .. } => {
                // 国内 LLM 适配暂不支持音频输入，跳过。
                tracing::warn!("Skipping ContentItem::InputAudio (unsupported by rig bridge)");
                None
            }
        })
        .collect()
}

fn convert_assistant_content(
    items: &[ContentItem],
    protocol: RigProtocol,
) -> Vec<AssistantContent> {
    items
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Some(AssistantContent::text(text.clone()))
            }
            ContentItem::InputImage { image, detail } => {
                let ImageReference::Inline { image_url } = image else {
                    return None;
                };
                image_from_url(image_url, *detail, protocol).map(AssistantContent::Image)
            }
            ContentItem::InputAudio { .. } => None,
        })
        .collect()
}
