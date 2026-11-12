use std::collections::HashMap;

use crate::config::ManagedFeatures;
use crate::context::ContextualUserFragment;
use crate::context::ImageResizeNotice;
use crate::context::ImageResizeNoticeSource;
use crate::context::ResizedImage;
use crate::original_image_detail::can_request_original_image_detail;
use codex_analytics::ImageDetailSetting;
use codex_analytics::ImagePreparationMetadata;
use codex_attachment_store::AttachmentStore;
use codex_attachment_store::UploadRequest;
use codex_attachment_store::UploadResult;
use codex_context_fragments::AnnotatedContent;
use codex_context_fragments::set_annotated_content;
use codex_context_fragments::to_annotated_content;
use codex_features::Feature;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_utils_image::EncodedImage;
use codex_utils_image::ImageProcessingError;
use codex_utils_image::PromptImageMode;
use codex_utils_image::data_url_from_bytes;
use codex_utils_image::fit_data_url_to_byte_budget;
use codex_utils_image::load_data_url_for_prompt;
use tracing::warn;

pub(crate) const IMAGE_PROCESSING_ERROR_PLACEHOLDER: &str =
    "image content omitted because it could not be processed";
/// Fork (nuwax-codex): per-image encoded budget used when the request path
/// cannot deliver `detail: original` pixels (FORK.md §5.5). A 2048-pixel
/// screenshot stays readable in well under 2 MiB; every byte above that is
/// discarded by the gateway.
pub(crate) const DEFAULT_DOWNSAMPLED_IMAGE_BUDGET_BYTES: usize = 2 * 1024 * 1024;

const UNSUPPORTED_LOW_DETAIL_PLACEHOLDER: &str = "image content omitted because detail 'low' is not supported; use 'high', 'original', or 'auto'";
const REMOTE_IMAGE_URL_PLACEHOLDER: &str =
    "image content omitted because remote image URLs are not supported";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImagePreparationMode {
    DetailBased,
    UnifiedBudget,
}

pub(crate) fn unified_image_budget_enabled(
    features: &ManagedFeatures,
    model_info: &ModelInfo,
) -> bool {
    features.enabled(Feature::UnifiedImageBudget)
        && (model_info.use_responses_lite || can_request_original_image_detail(model_info))
}

#[derive(Clone, Copy, Debug)]
struct ImageOrigin<'a> {
    thread_id: &'a str,
    ephemeral: bool,
    message_role: Option<&'a str>,
    item_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageResizeNoticeMode {
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparedImageResize {
    source_width: u32,
    source_height: u32,
    prepared_width: u32,
    prepared_height: u32,
}

pub(crate) struct PreparedInlineImage {
    encoded: EncodedImage,
    effective_detail: ImageDetailSetting,
    resize: Option<PreparedImageResize>,
}

impl PreparedInlineImage {
    pub(crate) fn into_data_url(self) -> String {
        self.encoded.into_data_url()
    }

    fn metadata(&self, origin: ImageOrigin<'_>) -> ImagePreparationMetadata {
        ImagePreparationMetadata {
            message_role: origin.message_role.map(str::to_string),
            item_id: origin.item_id.map(str::to_string),
            effective_detail: self.effective_detail,
            source_width: self.encoded.source_width,
            source_height: self.encoded.source_height,
            prepared_width: self.encoded.width,
            prepared_height: self.encoded.height,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ImagePreparationError {
    #[error("remote image URLs are not supported")]
    RemoteUrlUnsupported,
    #[error("image detail `low` is not supported")]
    UnsupportedLowDetail,
    #[error(transparent)]
    Processing(#[from] ImageProcessingError),
}

impl ImagePreparationError {
    pub(crate) const KIND: &str = "images.preparation_error";

    /// Fork (nuwax-codex): the placeholder carries the numbers that were
    /// actually hit. "exceeded the supported size limit" sent agents hunting
    /// for a byte threshold while the real cause was a pixel ceiling, and a
    /// model that cannot see the limit cannot ask for a smaller capture.
    fn placeholder(&self) -> String {
        match self {
            ImagePreparationError::RemoteUrlUnsupported => REMOTE_IMAGE_URL_PLACEHOLDER.to_string(),
            ImagePreparationError::UnsupportedLowDetail => {
                UNSUPPORTED_LOW_DETAIL_PLACEHOLDER.to_string()
            }
            ImagePreparationError::Processing(ImageProcessingError::ImagePixelsTooLarge {
                width,
                height,
                max,
                ..
            }) => format!(
                "image content omitted because it is {width}×{height} pixels, over the {max}-pixel \
                 prompt limit; capture a smaller region or a lower device pixel ratio"
            ),
            ImagePreparationError::Processing(ImageProcessingError::ImageTooLarge {
                size,
                max,
                ..
            }) => {
                format!(
                    "image content omitted because it is {size} bytes, over the {max}-byte prompt \
                     limit; capture a smaller image"
                )
            }
            ImagePreparationError::Processing(_) => IMAGE_PROCESSING_ERROR_PLACEHOLDER.to_string(),
        }
    }
}

pub(crate) async fn prepare_response_items(
    thread_id: &str,
    ephemeral: bool,
    items: &mut Vec<ResponseItem>,
    mode: ImagePreparationMode,
    resize_notice_mode: ImageResizeNoticeMode,
    image_store: &dyn AttachmentStore,
    // Fork (2026-11-11): the per-image byte budget travels with the call
    // instead of a process-wide cell. Two sessions with different configs used
    // to read whichever value was written last.
    byte_budget: Option<usize>,
) -> Vec<ImagePreparationMetadata> {
    let mut metadata = Vec::new();
    let mut prepared_items = Vec::with_capacity(items.len());
    for mut item in std::mem::take(items) {
        let mut annotated_content = to_annotated_content(&mut item);
        let resize_notice = match &mut item {
            ResponseItem::Message { role, .. } => {
                let Some(mut content) = annotated_content.take() else {
                    continue;
                };
                let resized_images = prepare_message_content(
                    &mut content,
                    ImageOrigin {
                        thread_id,
                        ephemeral,
                        message_role: Some(role.as_str()),
                        item_id: None,
                    },
                    if role.as_str() == "user" {
                        resize_notice_mode
                    } else {
                        ImageResizeNoticeMode::Disabled
                    },
                    &mut metadata,
                    mode,
                    image_store,
                    byte_budget,
                )
                .await;
                let _ = set_annotated_content(&mut item, content);
                (!resized_images.is_empty()).then(|| {
                    ImageResizeNotice::new(ImageResizeNoticeSource::UserMessage, resized_images)
                })
            }
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            } => {
                prepare_tool_output(
                    thread_id,
                    ephemeral,
                    output,
                    call_id.as_deref(),
                    resize_notice_mode,
                    &mut metadata,
                    mode,
                    image_store,
                    byte_budget,
                )
                .await
            }
            ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                prepare_tool_output(
                    thread_id,
                    ephemeral,
                    output,
                    Some(call_id.as_str()),
                    resize_notice_mode,
                    &mut metadata,
                    mode,
                    image_store,
                    byte_budget,
                )
                .await
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::ConfigurationUpdate { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => None,
        };
        prepared_items.push(item);
        if let Some(resize_notice) = resize_notice {
            prepared_items.push(ContextualUserFragment::into(resize_notice));
        }
    }
    *items = prepared_items;
    metadata
}

#[allow(clippy::too_many_arguments)]
async fn prepare_tool_output(
    thread_id: &str,
    ephemeral: bool,
    output: &mut FunctionCallOutputPayload,
    item_id: Option<&str>,
    resize_notice_mode: ImageResizeNoticeMode,
    metadata: &mut Vec<ImagePreparationMetadata>,
    mode: ImagePreparationMode,
    image_store: &dyn AttachmentStore,
    byte_budget: Option<usize>,
) -> Option<ImageResizeNotice> {
    let content = output.content_items_mut()?;
    let resized_images = prepare_tool_output_content(
        content,
        ImageOrigin {
            thread_id,
            ephemeral,
            message_role: None,
            item_id,
        },
        resize_notice_mode,
        metadata,
        mode,
        image_store,
        byte_budget,
    )
    .await;
    if resized_images.is_empty() {
        None
    } else {
        Some(ImageResizeNotice::new(
            ImageResizeNoticeSource::ToolOutput,
            resized_images,
        ))
    }
}

async fn prepare_message_content(
    items: &mut [AnnotatedContent],
    origin: ImageOrigin<'_>,
    resize_notice_mode: ImageResizeNoticeMode,
    metadata: &mut Vec<ImagePreparationMetadata>,
    mode: ImagePreparationMode,
    image_store: &dyn AttachmentStore,
    byte_budget: Option<usize>,
) -> Vec<ResizedImage> {
    // File-backed images are already prepared and pass through while retaining their positions in
    // resize notices.
    let image_count = items
        .iter()
        .filter(|item| matches!(item.content(), ContentItem::InputImage { .. }))
        .count();
    let mut image_number = 0;
    let mut resized_images = Vec::new();
    for item in items {
        if let ContentItem::InputImage { image, detail } = item.content_mut() {
            image_number += 1;
            match prepare_image(
                image,
                detail,
                origin,
                metadata,
                mode,
                image_store,
                byte_budget,
            )
            .await
            {
                Ok(Some(resize)) if resize_notice_mode == ImageResizeNoticeMode::Enabled => {
                    resized_images.push(ResizedImage {
                        image_number,
                        image_count,
                        source_width: resize.source_width,
                        source_height: resize.source_height,
                        prepared_width: resize.prepared_width,
                        prepared_height: resize.prepared_height,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%error, "failed to prepare message image");
                    *item = AnnotatedContent::text(
                        error.placeholder(),
                        ContentItemKind(ImagePreparationError::KIND.to_string()),
                        codex_protocol::models::ContentItemMetadata::harness(),
                    );
                }
            }
        }
    }
    resized_images
}

async fn prepare_tool_output_content(
    items: &mut [FunctionCallOutputContentItem],
    origin: ImageOrigin<'_>,
    resize_notice_mode: ImageResizeNoticeMode,
    metadata: &mut Vec<ImagePreparationMetadata>,
    mode: ImagePreparationMode,
    image_store: &dyn AttachmentStore,
    byte_budget: Option<usize>,
) -> Vec<ResizedImage> {
    // Submission-time backstop for the per-result image entrance limits:
    // every producer normally truncates through codex-utils-output-truncation
    // before items reach history, but dynamic tools and rehydrated rollouts
    // can arrive unbounded. Idempotent: already-limited payloads pass.
    enforce_tool_output_image_limits(items);
    let image_count = items
        .iter()
        .filter(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
        .count();
    let mut image_number = 0;
    let mut resized_images = Vec::new();
    for item in items {
        if let FunctionCallOutputContentItem::InputImage { image, detail } = item {
            image_number += 1;
            match prepare_image(
                image,
                detail,
                origin,
                metadata,
                mode,
                image_store,
                byte_budget,
            )
            .await
            {
                Ok(Some(resize)) if resize_notice_mode == ImageResizeNoticeMode::Enabled => {
                    resized_images.push(ResizedImage {
                        image_number,
                        image_count,
                        source_width: resize.source_width,
                        source_height: resize.source_height,
                        prepared_width: resize.prepared_width,
                        prepared_height: resize.prepared_height,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(%error, "failed to prepare tool output image");
                    *item = FunctionCallOutputContentItem::InputText {
                        text: error.placeholder(),
                    };
                }
            }
        }
    }
    resized_images
}

fn is_remote_image_url(image_url: &str) -> bool {
    image_url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

/// Submission-time backstop for the per-result image entrance limits. The
/// producer's truncation already applied them, so the fixed hard ceiling is
/// idempotent while still bounding legacy and dynamic outputs. Dropped
/// images become bounded text: the first carries the marker, the rest empty
/// slots (a slice cannot shrink; the backstop path is rare by design).
fn enforce_tool_output_image_limits(items: &mut [FunctionCallOutputContentItem]) {
    let admission = codex_utils_output_truncation::admit_tool_output_images(
        items,
        codex_protocol::protocol::TruncationPolicy::Tokens(usize::MAX),
    );
    if admission.dropped == 0 {
        return;
    }
    let mut marker_written = false;
    for (index, item) in items.iter_mut().enumerate() {
        if matches!(item, FunctionCallOutputContentItem::InputImage { .. })
            && !admission.keep[index]
        {
            *item = if marker_written {
                FunctionCallOutputContentItem::InputText {
                    text: String::new(),
                }
            } else {
                marker_written = true;
                codex_utils_output_truncation::omitted_images_marker(admission.dropped)
            };
        }
    }
}

fn is_data_url(image_url: &str) -> bool {
    image_url
        .get(.."data:".len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
}

/// The configured per-image byte budget, or `None` when disabled.
///
/// Fork (2026-11-11): this used to be a process-wide `AtomicU32` written by
/// every config load, so two sessions with different values clobbered each
/// other and a failing load still changed it. It now travels with the call,
/// read from the session's own config.
pub(crate) fn image_byte_budget(
    config: &crate::config::Config,
    model_info: &codex_protocol::openai_models::ModelInfo,
) -> Option<usize> {
    match config.image_byte_budget_bytes {
        // Explicit number.
        Some(bytes) if bytes > 0 => Some(bytes as usize),
        // Explicit off switch.
        Some(_) => None,
        // Fork (nuwax-codex): budget images whose extra detail the wire throws
        // away. See `wire_honors_original_detail`.
        None if !wire_honors_original_detail(config, model_info) => {
            Some(DEFAULT_DOWNSAMPLED_IMAGE_BUDGET_BYTES)
        }
        None => None,
    }
}

/// Whether the request path can actually deliver a `detail: original` image's
/// pixels to the provider.
///
/// Fork (nuwax-codex): the model catalog's `supports_image_detail_original` is
/// only half the answer. On the Chat wire the bridge rewrites `original` to
/// `high` unconditionally (`request_content::image_detail`), and the Anthropic
/// wire has no detail field at all — the provider rescales to its own ceiling.
/// Sending a 6000 px origin on either wire means paying for bytes nobody reads,
/// so only a Responses wire with model support counts as honouring the pixels.
fn wire_honors_original_detail(
    config: &crate::config::Config,
    model_info: &codex_protocol::openai_models::ModelInfo,
) -> bool {
    wire_honors_original_detail_for(
        config.model_provider.wire_api,
        model_info.supports_image_detail_original,
    )
}

fn wire_honors_original_detail_for(
    wire_api: codex_model_provider_info::WireApi,
    supports_original: bool,
) -> bool {
    matches!(wire_api, codex_model_provider_info::WireApi::Responses) && supports_original
}

async fn prepare_image(
    image: &mut ImageReference,
    detail: &mut Option<ImageDetail>,
    origin: ImageOrigin<'_>,
    metadata: &mut Vec<ImagePreparationMetadata>,
    mode: ImagePreparationMode,
    image_store: &dyn AttachmentStore,
    byte_budget: Option<usize>,
) -> Result<Option<PreparedImageResize>, ImagePreparationError> {
    let ImageReference::Inline { image_url } = image else {
        return Ok(None);
    };
    let Some(prepared) = resize_image(image_url, detail, mode, byte_budget)? else {
        return Ok(None);
    };
    metadata.push(prepared.metadata(origin));
    let resize = prepared.resize;
    // Tool outputs have no message role. Keep their original-detail bytes
    // inspectable: uploading to a File reference would discard the dimensions
    // needed by the shared budget on the next history pass. User messages and
    // other detail levels retain the attachment store's normal upload policy.
    if origin.message_role.is_none() && *detail == Some(ImageDetail::Original) {
        *image = ImageReference::Inline {
            image_url: prepared.into_data_url(),
        };
        return Ok(resize);
    }
    let request = UploadRequest {
        thread_id: origin.thread_id.to_owned(),
        ephemeral: origin.ephemeral,
        file_name: None,
        tags: HashMap::new(),
        data: prepared.encoded.bytes.to_vec(),
    };
    *image = match image_store.upload(request).await {
        Ok(UploadResult::Inline { bytes }) => ImageReference::Inline {
            image_url: data_url_from_bytes(&prepared.encoded.mime, &bytes),
        },
        Ok(UploadResult::File { file_id }) => ImageReference::File { file_id },
        Err(error) => {
            warn!(kind = ?error.kind(), "failed to upload prepared image");
            ImageReference::Inline {
                image_url: prepared.into_data_url(),
            }
        }
    };
    Ok(resize)
}

pub(crate) fn resize_image(
    image_url: &str,
    detail: &mut Option<ImageDetail>,
    mode: ImagePreparationMode,
    byte_budget: Option<usize>,
) -> Result<Option<PreparedInlineImage>, ImagePreparationError> {
    if is_remote_image_url(image_url) {
        return Err(ImagePreparationError::RemoteUrlUnsupported);
    }
    if !is_data_url(image_url) {
        return Ok(None);
    }

    let (effective_detail, image_mode) = match mode {
        ImagePreparationMode::UnifiedBudget => (
            ImageDetailSetting::Original,
            PromptImageMode::ORIGINAL_DETAIL,
        ),
        ImagePreparationMode::DetailBased => match detail {
            None | Some(ImageDetail::Auto | ImageDetail::High) => {
                (ImageDetailSetting::High, PromptImageMode::HIGH_DETAIL)
            }
            Some(ImageDetail::Original) => (
                ImageDetailSetting::Original,
                PromptImageMode::ORIGINAL_DETAIL,
            ),
            Some(ImageDetail::Low) => return Err(ImagePreparationError::UnsupportedLowDetail),
        },
    };
    let mut encoded = load_data_url_for_prompt(image_url, image_mode)?;
    // Fork (2026-11-10): bound the encoded payload to the configured byte
    // budget before it enters history. Lossless-first (see
    // `fit_encoded_to_budget`): opaque images flatten to a JPEG ladder,
    // genuinely transparent ones keep PNG and shrink by dimension. Every
    // later turn re-sends history, so a small image here pays off forever.
    if let Some(max_bytes) = byte_budget
        && encoded.bytes.len() > max_bytes
    {
        let (_fitted_url, fitted_bytes, fitted_mime) =
            fit_data_url_to_byte_budget(image_url, max_bytes)?;
        // Fork (2026-11-11): the ladder may resize as well as re-encode, so the
        // reported dimensions have to come from the bytes we are actually about
        // to send. Keeping the pre-fit width/height made the resize notice (and
        // anything mapping screenshot coordinates) describe an image that no
        // longer exists.
        if let Some((width, height)) = codex_utils_image::header_dimensions(&fitted_bytes) {
            encoded.width = width;
            encoded.height = height;
        }
        encoded.bytes = fitted_bytes.into();
        encoded.mime = fitted_mime;
    }
    let resize = ((encoded.source_width, encoded.source_height) != (encoded.width, encoded.height))
        .then_some(PreparedImageResize {
            source_width: encoded.source_width,
            source_height: encoded.source_height,
            prepared_width: encoded.width,
            prepared_height: encoded.height,
        });
    if mode == ImagePreparationMode::UnifiedBudget {
        // Preserve accurate context-window accounting while older transports still require an
        // image detail field. Responses Lite removes this compatibility hint before sending.
        *detail = Some(ImageDetail::Original);
    }
    Ok(Some(PreparedInlineImage {
        encoded,
        effective_detail,
        resize,
    }))
}

#[cfg(test)]
#[path = "image_preparation_tests.rs"]
pub(crate) mod tests;
