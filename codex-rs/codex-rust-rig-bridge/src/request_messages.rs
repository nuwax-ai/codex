//! History conversion shared by the two Rig wire protocols.
use crate::client::RigProtocol;
use crate::request_content::append_tool_result;
use crate::request_content::convert_assistant_content;
use crate::request_content::convert_user_content;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use rig_core::completion::message::AssistantContent;
use rig_core::completion::message::Message;
use serde_json::Value;

pub(crate) fn convert_response_items(
    items: &[ResponseItem],
    protocol: RigProtocol,
    source: &str,
    websearch_replay: &mut Vec<crate::hosted_replay::ReplayGroup>,
) -> Result<Vec<Message>, codex_api::ApiError> {
    let mut messages: Vec<Message> = Vec::new();
    // How many assistant rig messages have been emitted so far; persisted
    // web-search pairs attach to the message their call followed (count-1).
    let mut assistant_messages = 0usize;

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

    // R3: a mixed server/client turn closes its pending search with a NEW
    // appended completed item; the earlier in_progress item (and any later
    // duplicate) for the same server call must not replay twice. Select the
    // first valid completed ITEM, not an ID set that also removes the winner.
    let mut replay_envelopes: Vec<_> = items
        .iter()
        .map(|item| match (protocol, item) {
            (
                RigProtocol::Anthropic,
                ResponseItem::WebSearchCall {
                    wire_blocks: Some(payload),
                    ..
                },
            ) => crate::hosted_replay::validated_envelope(payload, source),
            _ => None,
        })
        .collect();
    let mut first_completed_search = std::collections::HashMap::<String, usize>::new();
    for (index, envelope) in replay_envelopes.iter().enumerate() {
        let Some(envelope) = envelope else { continue };
        for call in envelope
            .blocks
            .iter()
            .filter(|block| block["type"] == "server_tool_use")
        {
            let Some(id) = call.get("id").and_then(Value::as_str) else {
                continue;
            };
            if envelope
                .blocks
                .iter()
                .any(|block| block.get("tool_use_id").and_then(Value::as_str) == Some(id))
            {
                first_completed_search
                    .entry(id.to_string())
                    .or_insert(index);
            }
        }
    }
    // Calls whose server_tool_use block is already projected at its ORIGINAL
    // history position (a superseded in_progress item): the winning completed
    // item then contributes only its result block at the new response
    // position, so the request prefix never moves a call.
    let mut projected_calls = std::collections::HashSet::<String>::new();

    for (item_index, item) in items.iter().enumerate() {
        match item {
            ResponseItem::FunctionCall { call_id, .. }
            | ResponseItem::CustomToolCall { call_id, .. }
            | ResponseItem::FunctionCallOutput {
                call_id: Some(call_id),
                ..
            }
            | ResponseItem::CustomToolCallOutput { call_id, .. }
                if call_id.is_empty() =>
            {
                // Rig mints a fresh ID for each empty string, which would
                // disconnect saved calls from their outputs on every replay.
                return Err(codex_api::ApiError::InvalidRequest {
                    message: "Historical tool call IDs must not be empty".into(),
                });
            }
            ResponseItem::Message { role, content, .. } => {
                let role = role.as_str();
                if role == "assistant" {
                    let parts = convert_assistant_content(content, protocol)?;
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
                            _ => {
                                messages.push(Message::Assistant {
                                    id: None,
                                    content: parts,
                                });
                                assistant_messages += 1;
                            }
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
                    let parts = convert_user_content(content, protocol)?;
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
                for reasoning in crate::reasoning::replay_reasoning(item, source, protocol) {
                    match messages.last_mut() {
                        Some(Message::Assistant { content, .. }) => {
                            content.push(AssistantContent::Reasoning(reasoning));
                        }
                        _ => {
                            messages.push(Message::Assistant {
                                id: None,
                                content: vec![AssistantContent::Reasoning(reasoning)],
                            });
                            assistant_messages += 1;
                        }
                    }
                }
            }
            ResponseItem::WebSearchCall { .. } => {
                // Fork (nuwax-codex): persisted raw wire pairs replay into
                // the assistant message they followed — Anthropic wire only,
                // and only when the payload was captured by the same source
                // identity sending this request (legacy bare arrays and
                // cross-endpoint ciphertext are conservatively dropped).
                match replay_envelopes[item_index].take() {
                    Some(mut envelope) => {
                        // Result-only projections still need a block Rig can
                        // serialize. Keep the original validated call for the
                        // SDK anchor; transport removes it before raw replay.
                        let anchor = envelope
                            .blocks
                            .iter()
                            .find(|block| block["type"] == "server_tool_use")
                            .cloned();
                        envelope
                            .blocks
                            .retain(|block| match block["type"].as_str() {
                                Some("server_tool_use") => {
                                    // The use stays at its original position —
                                    // including superseded pending items — so the
                                    // request prefix is stable; a winning item
                                    // contributes its call only when no earlier
                                    // item already projected it.
                                    let keep =
                                        block.get("id").and_then(Value::as_str).is_some_and(|id| {
                                            !projected_calls.contains(id)
                                                && first_completed_search
                                                    .get(id)
                                                    .is_none_or(|winner| *winner >= item_index)
                                        });
                                    if keep
                                        && let Some(id) = block.get("id").and_then(Value::as_str)
                                    {
                                        projected_calls.insert(id.to_string());
                                    }
                                    keep
                                }
                                Some("web_search_tool_result" | "tool_result") => {
                                    // Results replay only from the winning item.
                                    block
                                        .get("tool_use_id")
                                        .and_then(Value::as_str)
                                        .is_some_and(|id| {
                                            first_completed_search
                                                .get(id)
                                                .is_some_and(|winner| *winner == item_index)
                                        })
                                }
                                _ => true,
                            });
                        let blocks = envelope.blocks;
                        let cited_text = envelope.cited_text;
                        if blocks.is_empty() {
                            tracing::warn!(
                                "persisted web-search pair carries no blocks; dropping it"
                            );
                            continue;
                        }
                        if !matches!(messages.last(), Some(Message::Assistant { .. })) {
                            let Some(anchor) = anchor else {
                                continue;
                            };
                            messages.push(Message::Assistant {
                                id: None,
                                // Rig rejects empty assistant messages before HTTP injection.
                                // Carry a genuine raw block through its supported raw-content
                                // channel; transport replaces this anchor with the payload.
                                content: vec![AssistantContent::Text(rig_core::completion::message::Text {
                                    text: String::new(),
                                    additional_params: rig_core::completion::message::AdditionalParams::from_entries([
                                        ("anthropic_content", anchor),
                                    ]),
                                })],
                            });
                            assistant_messages += 1;
                        }
                        let target = assistant_messages - 1;
                        match websearch_replay.last_mut() {
                            Some(group) if group.index == target => {
                                group.blocks.extend(blocks);
                                group.cited_text.extend(cited_text);
                            }
                            _ => websearch_replay.push(crate::hosted_replay::ReplayGroup {
                                index: target,
                                blocks,
                                cited_text,
                            }),
                        }
                    }
                    None => {
                        tracing::debug!(
                            "web-search history lacks a valid same-source replay payload; dropping"
                        );
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
                // Preserve history: invalid calls must be diagnosed rather than
                // silently removed together with their results or coerced by Rig.
                let args = if arguments.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    match serde_json::from_str::<Value>(arguments) {
                        Ok(args @ Value::Object(_)) => args,
                        Ok(_) => {
                            return Err(codex_api::ApiError::InvalidRequest {
                                message: format!(
                                    "Historical tool {name} ({call_id}) arguments must be a JSON object"
                                ),
                            });
                        }
                        Err(error) => {
                            return Err(codex_api::ApiError::InvalidRequest {
                                message: format!(
                                    "Invalid historical arguments for tool {name} ({call_id}): {error}"
                                ),
                            });
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
                    _ => {
                        messages.push(Message::Assistant {
                            id: None,
                            content: vec![part],
                        });
                        assistant_messages += 1;
                    }
                }
            }
            ResponseItem::FunctionCallOutput {
                call_id,
                output,
                name,
                namespace,
                ..
            } => {
                let Some(call_id) = call_id else {
                    crate::request_content::append_external_output(
                        &mut messages,
                        name.as_deref(),
                        namespace.as_deref(),
                        output,
                    )?;
                    continue;
                };
                append_tool_result(&mut messages, call_id, output, &tool_names, protocol)?;
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
                    _ => {
                        messages.push(Message::Assistant {
                            id: None,
                            content: vec![part],
                        });
                        assistant_messages += 1;
                    }
                }
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                append_tool_result(&mut messages, call_id, output, &tool_names, protocol)?;
            }
            // Internal/local events — not sent to the model.
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
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
                        _ => {
                            messages.push(Message::Assistant {
                                id: None,
                                content: vec![AssistantContent::text(text)],
                            });
                            assistant_messages += 1;
                        }
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

#[cfg(test)]
#[path = "request_messages_tests.rs"]
mod tests;
