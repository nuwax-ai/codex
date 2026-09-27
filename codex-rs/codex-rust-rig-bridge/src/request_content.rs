//! Protocol content conversion and bounded tool results.
use crate::RigProtocol;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::ImageReference;
use rig_core::completion::message::AssistantContent;
use rig_core::completion::message::DocumentSourceKind;
use rig_core::completion::message::Image as RigImage;
use rig_core::completion::message::ImageMediaType;
use rig_core::completion::message::Message;
use rig_core::completion::message::ToolResultContent;
use rig_core::completion::message::UserContent;
pub(crate) fn append_tool_result(
    messages: &mut Vec<Message>,
    call_id: &str,
    output: &codex_protocol::models::FunctionCallOutputPayload,
    tool_names: &std::collections::HashMap<String, String>,
    protocol: RigProtocol,
) -> Result<(), codex_api::ApiError> {
    let parts = convert_tool_output(output)?;
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
    Ok(())
}

/// A standalone external event has no preceding tool call. Keep its attributed
/// content as user input instead of fabricating an orphan tool_result.
pub(crate) fn append_external_output(
    messages: &mut Vec<Message>,
    name: Option<&str>,
    namespace: Option<&str>,
    output: &codex_protocol::models::FunctionCallOutputPayload,
) -> Result<(), codex_api::ApiError> {
    let name = crate::request_tools::flat_name(name.unwrap_or("external event"), namespace);
    let name = codex_utils_string::truncate_middle_with_token_budget(&name, 100).0;
    let mut content = vec![UserContent::text(format!("External event from {name}:"))];
    content.extend(
        convert_tool_output(output)?
            .into_iter()
            .map(|part| match part {
                ToolResultContent::Text(text) => UserContent::Text(text),
                ToolResultContent::Image(image) => UserContent::Image(image),
                ToolResultContent::Json { value } => UserContent::text(value.to_string()),
            }),
    );
    messages.push(Message::User { content });
    Ok(())
}

/// Bound the combined text of each result, with space for truncation markers
/// and source attribution inside the repository's approximate 10k token cap.
fn convert_tool_output(
    output: &codex_protocol::models::FunctionCallOutputPayload,
) -> Result<Vec<ToolResultContent>, codex_api::ApiError> {
    use codex_protocol::models::FunctionCallOutputContentItem;
    let mut remaining = 9_800;
    let mut omitted_tail_marked = false;
    let mut text_part = |text: &str| {
        if remaining == 0 {
            if !text.is_empty() && !omitted_tail_marked {
                omitted_tail_marked = true;
                return Some(ToolResultContent::text(
                    "[Additional tool output truncated]",
                ));
            }
            return None;
        }
        let text = codex_utils_string::truncate_middle_with_token_budget(text, remaining).0;
        remaining = remaining.saturating_sub(codex_utils_string::approx_token_count(&text));
        Some(ToolResultContent::text(text))
    };
    let items = match &output.body {
        FunctionCallOutputBody::Text(text) => return Ok(text_part(text).into_iter().collect()),
        FunctionCallOutputBody::ContentItems(items) => items,
    };
    let mut parts = Vec::new();
    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => {
                parts.extend(text_part(text));
            }
            FunctionCallOutputContentItem::InputImage { image, detail } => match image {
                ImageReference::Inline { image_url } => {
                    parts.push(ToolResultContent::Image(image_from_url(image_url, *detail)?));
                }
                ImageReference::File { .. } => return Err(codex_api::ApiError::InvalidRequest {
                    message: "File-ID images require native Responses or inline image data; the Rig bridge cannot resolve provider-scoped file IDs".into(),
                }),
            },
            FunctionCallOutputContentItem::InputAudio { .. } => {
                return Err(codex_api::ApiError::InvalidRequest {
                    message: "Audio tool results are unsupported by the Rig bridge".into(),
                });
            }
            FunctionCallOutputContentItem::EncryptedContent { .. } => {
                // Provider-encrypted content cannot be replayed through a
                // different provider; a placeholder keeps the tool result
                // visible to the model without leaking opaque bytes.
                parts.extend(text_part("(encrypted content omitted)"));
            }
        }
    }
    if parts.is_empty() {
        parts.push(ToolResultContent::text("(empty tool result)"));
    }
    Ok(parts)
}

/// Parses a `data:<mime>;base64,<payload>` URL into a base64-backed image.
/// Returns `None` for any data URL the bridge cannot decode (unsupported
/// mime, missing base64 marker). The Anthropic wire renders URL sources as
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
) -> Result<RigImage, codex_api::ApiError> {
    use codex_protocol::models::ImageDetail as CodexDetail;
    use rig_core::completion::message::ImageDetail as RigDetail;
    let parsed = if url.starts_with("data:") {
        match data_url_image(url) {
            Some(image) => Some(image),
            None => {
                return Err(codex_api::ApiError::InvalidRequest {
                    message: "Unsupported inline image: use base64 JPEG, PNG, GIF or WebP".into(),
                });
            }
        }
    } else if reqwest_rig::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
    {
        None
    } else {
        return Err(codex_api::ApiError::InvalidRequest {
            message: "Image URL must use HTTP(S) or a supported base64 data URL".into(),
        });
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
    Ok(image)
}

pub(crate) fn convert_user_content(
    items: &[ContentItem],
    _protocol: RigProtocol,
) -> Result<Vec<UserContent>, codex_api::ApiError> {
    items.iter().map(|item| match item {
        ContentItem::InputText { text } | ContentItem::OutputText { text } => Ok(UserContent::text(text.clone())),
        ContentItem::InputImage { image, detail } => {
            let ImageReference::Inline { image_url } = image else {
                return Err(codex_api::ApiError::InvalidRequest {
                    message: "File-ID images require native Responses or inline image data; the Rig bridge cannot resolve provider-scoped file IDs".into(),
                });
            };
            image_from_url(image_url, *detail).map(UserContent::Image)
        }
        ContentItem::InputAudio { .. } => Err(codex_api::ApiError::InvalidRequest {
            message: "Audio input is unsupported by the Rig bridge".into(),
        }),
    }).collect()
}

pub(crate) fn convert_assistant_content(
    items: &[ContentItem],
    _protocol: RigProtocol,
) -> Result<Vec<AssistantContent>, codex_api::ApiError> {
    items
        .iter()
        .map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                Ok(AssistantContent::text(text.clone()))
            }
            ContentItem::InputImage { .. } => Err(codex_api::ApiError::InvalidRequest {
                message: "Assistant image history is unsupported by the Rig Chat/Messages bridge"
                    .into(),
            }),
            ContentItem::InputAudio { .. } => Err(codex_api::ApiError::InvalidRequest {
                message: "Assistant audio history is unsupported by the Rig bridge".into(),
            }),
        })
        .collect()
}
