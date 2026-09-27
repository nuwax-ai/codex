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

    for item in items {
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
                    _ => messages.push(Message::Assistant {
                        id: None,
                        content: vec![part],
                    }),
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
                    _ => messages.push(Message::Assistant {
                        id: None,
                        content: vec![part],
                    }),
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
