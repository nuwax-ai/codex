use std::io::Cursor;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use codex_attachment_store::AttachmentStoreError;
use codex_attachment_store::AttachmentStoreErrorKind;
use codex_attachment_store::InlineAttachmentStore;
use codex_attachment_store::ResolveFuture;
use codex_attachment_store::ResolveRequest;
use codex_attachment_store::UploadFuture;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageReference;
use codex_protocol::models::InternalChatMessageMetadataPassthrough;
use codex_utils_image::data_url_from_bytes;
use image::DynamicImage;
use image::GenericImageView;
use image::ImageBuffer;
use image::ImageFormat;
use image::Rgba;
use pretty_assertions::assert_eq;
use serde_json::from_value;
use serde_json::json;

use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_by_two_original_tool_image_reaches_prepared_history_with_detail_intact() {
    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=";
    let image_store = RecordingFileAttachmentStore::default();
    let mut envelopes = vec![crate::tools::registry::AnyToolResult {
        call_id: "original-shot".to_string(),
        payload: codex_tools::ToolPayload::Function { arguments: "{}".to_string() },
        result: Box::new(crate::tools::context::McpToolOutput {
            result: codex_protocol::mcp::CallToolResult {
                content: vec![serde_json::json!({"type": "image", "mimeType": "image/png", "data": image_url, "_meta": {"codex/imageDetail": "original"}})],
                structured_content: None,
                is_error: Some(false),
                meta: None,
            },
            tool_input: serde_json::json!({}),
            result_metadata_capture_allowed: false,
            wall_time: std::time::Duration::from_millis(/*millis*/ 100),
            original_image_detail_supported: true,
            truncation_policy: codex_utils_output_truncation::TruncationPolicy::Tokens(32),
        }),
        post_tool_use_payload: None,
    }.into_response()];
    let mut items = vec![envelopes[0].item.clone()];
    prepare_response_items(
        "thread-original",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &image_store,
    )
    .await;
    assert_eq!(
        *image_store.uploads.lock().expect("uploads lock"),
        Vec::<UploadRequest>::new()
    );
    envelopes[0].item = items.remove(0);
    let expected = envelopes[0].item.clone();
    let mut history = crate::context_manager::ContextManager::new();
    history.record_annotated_items(
        &mut envelopes,
        codex_utils_output_truncation::TruncationPolicy::Tokens(10_000),
    );
    assert_eq!(
        history.raw_items().cloned().collect::<Vec<_>>(),
        vec![expected.clone()]
    );
    let ResponseItem::FunctionCallOutput { output, .. } = expected else {
        panic!("original tool output");
    };
    assert_eq!(
        output.content_items().expect("typed image")[1],
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: image_url.to_string()
            },
            detail: Some(ImageDetail::Original),
        }
    );
}

#[derive(Default)]
struct RecordingFileAttachmentStore {
    uploads: std::sync::Mutex<Vec<UploadRequest>>,
}

impl AttachmentStore for RecordingFileAttachmentStore {
    fn upload(&self, request: UploadRequest) -> UploadFuture<'_> {
        self.uploads.lock().expect("uploads lock").push(request);
        Box::pin(async {
            Ok(UploadResult::File {
                file_id: "file-uploaded".to_string(),
            })
        })
    }

    fn resolve<'a>(&'a self, request: ResolveRequest<'a>) -> ResolveFuture<'a> {
        let file_id = request.file_id;
        Box::pin(async move {
            Err(AttachmentStoreError::new(
                AttachmentStoreErrorKind::NotFound,
                format!("attachment `{file_id}` was not found"),
            ))
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_original_and_high_tool_images_keep_the_file_store_upload_policy() {
    let (image_url, bytes) = png_data_url(/*width*/ 2, /*height*/ 2);
    let image_store = RecordingFileAttachmentStore::default();
    let mut items = vec![
        from_value::<ResponseItem>(json!({"type": "message", "role": "user", "content": [{"type": "input_image", "image_url": image_url, "detail": "original"}]})).expect("user image"),
        from_value::<ResponseItem>(json!({"type": "function_call_output", "call_id": "high-shot", "output": [{"type": "input_image", "image_url": image_url, "detail": "high"}]})).expect("tool image"),
    ];
    prepare_response_items(
        "thread-upload",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &image_store,
    )
    .await;
    let references: Vec<_> = items
        .iter()
        .map(|item| match item {
            ResponseItem::Message { content, .. } => match &content[0] {
                ContentItem::InputImage { image, detail } => (image.clone(), *detail),
                other => panic!("expected user image, got {other:?}"),
            },
            ResponseItem::FunctionCallOutput { output, .. } => {
                match &output.content_items().expect("tool content")[0] {
                    FunctionCallOutputContentItem::InputImage { image, detail } => {
                        (image.clone(), *detail)
                    }
                    other => panic!("expected tool image, got {other:?}"),
                }
            }
            other => panic!("unexpected prepared item: {other:?}"),
        })
        .collect();
    assert_eq!(
        references,
        vec![
            (
                ImageReference::File {
                    file_id: "file-uploaded".to_string()
                },
                Some(ImageDetail::Original)
            ),
            (
                ImageReference::File {
                    file_id: "file-uploaded".to_string()
                },
                Some(ImageDetail::High)
            ),
        ]
    );
    let request = UploadRequest {
        thread_id: "thread-upload".to_string(),
        ephemeral: false,
        file_name: None,
        tags: std::collections::HashMap::new(),
        data: bytes,
    };
    assert_eq!(
        *image_store.uploads.lock().expect("uploads lock"),
        vec![request.clone(), request]
    );
}

struct FailingAttachmentStore;

impl AttachmentStore for FailingAttachmentStore {
    fn upload(&self, _request: UploadRequest) -> UploadFuture<'_> {
        Box::pin(async {
            Err(AttachmentStoreError::new(
                AttachmentStoreErrorKind::Backend,
                "upload failed",
            ))
        })
    }

    fn resolve<'a>(&'a self, request: ResolveRequest<'a>) -> ResolveFuture<'a> {
        let file_id = request.file_id;
        Box::pin(async move {
            Err(AttachmentStoreError::new(
                AttachmentStoreErrorKind::NotFound,
                format!("attachment `{file_id}` was not found"),
            ))
        })
    }
}

fn png_data_url(width: u32, height: u32) -> (String, Vec<u8>) {
    let image = ImageBuffer::from_pixel(width, height, Rgba([10u8, 20, 30, 255]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut encoded, ImageFormat::Png)
        .expect("encode PNG");
    let bytes = encoded.into_inner();
    (data_url_from_bytes("image/png", &bytes), bytes)
}

fn decoded_image(image_url: &str) -> (Vec<u8>, DynamicImage) {
    let (_, payload) = image_url.split_once(',').expect("data URL payload");
    let bytes = BASE64_STANDARD.decode(payload).expect("decode image URL");
    let image = image::load_from_memory(&bytes).expect("decode processed image");
    (bytes, image)
}

#[tokio::test(flavor = "multi_thread")]
async fn preparation_preserves_small_image_bytes_and_replaces_remote_urls() {
    let (data_url, original_bytes) = png_data_url(/*width*/ 64, /*height*/ 32);
    let mut items = vec![ResponseItem::Message {
        status: None,
        encrypted_content: None,
        id: None,
        role: "user".to_string(),
        content: vec![
            ContentItem::InputImage {
                image: ImageReference::Inline {
                    image_url: data_url,
                },
                detail: Some(ImageDetail::High),
            },
            ContentItem::InputImage {
                image: ImageReference::Inline {
                    image_url: "https://example.com/image.png".to_string(),
                },
                detail: Some(ImageDetail::Low),
            },
        ],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }];
    items.push(ResponseItem::Message {
        status: None,
        encrypted_content: None,
        id: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "https://example.com/developer-image.png".to_string(),
            },
            detail: Some(ImageDetail::High),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });

    prepare_response_items(
        "image-preparation-thread",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &InlineAttachmentStore,
    )
    .await;

    let ResponseItem::Message { content, .. } = &items[0] else {
        panic!("expected message");
    };
    let [
        ContentItem::InputImage {
            image: ImageReference::Inline { image_url },
            ..
        },
        ContentItem::InputText { text },
    ] = content.as_slice()
    else {
        panic!("expected two images");
    };
    assert_eq!(decoded_image(image_url).0, original_bytes);
    assert_eq!(text, REMOTE_IMAGE_URL_PLACEHOLDER);
    assert_eq!(
        &items[1],
        &ResponseItem::Message {
            status: None,
            encrypted_content: None,
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: REMOTE_IMAGE_URL_PLACEHOLDER.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    content_item_kinds: Some(vec![ContentItemKind(
                        "images.preparation_error".to_string()
                    )]),
                    content_item_metadata: from_value(json!([{"harness_injected": true}])).unwrap(),
                    ..Default::default()
                }
            ),
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn detail_policies_apply_the_expected_budgets() {
    for (detail, effective_detail, input_dimensions, expected_dimensions) in [
        (
            Some(ImageDetail::High),
            ImageDetailSetting::High,
            (2048, 2048),
            (1600, 1600),
        ),
        (
            Some(ImageDetail::Original),
            ImageDetailSetting::Original,
            (6401, 100),
            (6000, 94),
        ),
        (
            Some(ImageDetail::Original),
            ImageDetailSetting::Original,
            (3201, 3201),
            (3200, 3200),
        ),
        (
            Some(ImageDetail::Auto),
            ImageDetailSetting::High,
            (2048, 2048),
            (1600, 1600),
        ),
        (None, ImageDetailSetting::High, (2048, 2048), (1600, 1600)),
    ] {
        let (image_url, _) = png_data_url(input_dimensions.0, input_dimensions.1);
        let mut items = vec![ResponseItem::Message {
            status: None,
            encrypted_content: None,
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputImage {
                image: ImageReference::Inline { image_url },
                detail,
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }];

        let metadata = prepare_response_items(
            "image-preparation-thread",
            /*ephemeral*/ false,
            &mut items,
            ImagePreparationMode::DetailBased,
            ImageResizeNoticeMode::Disabled,
            &InlineAttachmentStore,
        )
        .await;

        let ResponseItem::Message { content, .. } = &items[0] else {
            panic!("expected message");
        };
        let [
            ContentItem::InputImage {
                image: ImageReference::Inline { image_url },
                ..
            },
        ] = content.as_slice()
        else {
            panic!("expected image");
        };
        assert_eq!(decoded_image(image_url).1.dimensions(), expected_dimensions);
        assert_eq!(
            metadata,
            vec![ImagePreparationMetadata {
                message_role: Some("user".to_string()),
                item_id: None,
                effective_detail,
                source_width: input_dimensions.0,
                source_height: input_dimensions.1,
                prepared_width: expected_dimensions.0,
                prepared_height: expected_dimensions.1,
            }]
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn preparation_reports_tool_output_item_id() {
    let call_id = "call-image";
    let (image_url, _) = png_data_url(/*width*/ 64, /*height*/ 32);
    let mut items = vec![ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id.to_string()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload::from_content_items(vec![
            FunctionCallOutputContentItem::InputImage {
                image: ImageReference::Inline { image_url },
                detail: Some(ImageDetail::High),
            },
        ]),
        internal_chat_message_metadata_passthrough: None,
    }];
    let metadata = prepare_response_items(
        "image-preparation-thread",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &InlineAttachmentStore,
    )
    .await;

    assert_eq!(
        metadata,
        vec![ImagePreparationMetadata {
            message_role: None,
            item_id: Some(call_id.to_string()),
            effective_detail: ImageDetailSetting::High,
            source_width: 64,
            source_height: 32,
            prepared_width: 64,
            prepared_height: 32,
        }]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_failure_keeps_resized_image_inline() {
    let (image_url, _) = png_data_url(/*width*/ 2048, /*height*/ 2048);
    let mut items = vec![ResponseItem::Message {
        status: None,
        encrypted_content: None,
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputImage {
            image: ImageReference::Inline { image_url },
            detail: Some(ImageDetail::High),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }];

    prepare_response_items(
        "image-preparation-thread",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &FailingAttachmentStore,
    )
    .await;

    let ResponseItem::Message { content, .. } = &items[0] else {
        panic!("expected message");
    };
    let [
        ContentItem::InputImage {
            image: ImageReference::Inline { image_url },
            ..
        },
    ] = content.as_slice()
    else {
        panic!("expected inline image");
    };
    assert_eq!(decoded_image(image_url).1.dimensions(), (1600, 1600));
}

/// File-backed images retain their sequence positions even though preparation skips them.
#[tokio::test(flavor = "multi_thread")]
async fn resize_notices_count_file_backed_images_and_skip_failed_images() {
    let (large_image_url, _) = png_data_url(/*width*/ 2048, /*height*/ 2048);
    let (small_image_url, _) = png_data_url(/*width*/ 64, /*height*/ 32);
    let mut items = vec![
        ResponseItem::Message {
            status: None,
            encrypted_content: None,
            id: None,
            role: "user".to_string(),
            content: vec![
                ContentItem::InputImage {
                    image: ImageReference::File {
                        file_id: "file_message".to_string(),
                    },
                    detail: Some(ImageDetail::High),
                },
                ContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: small_image_url,
                    },
                    detail: Some(ImageDetail::High),
                },
                ContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,%%%".to_string(),
                    },
                    detail: Some(ImageDetail::High),
                },
                ContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: large_image_url.clone(),
                    },
                    detail: Some(ImageDetail::High),
                },
            ],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    content_item_kinds: Some(vec![
                        ContentItemKind("user.image".to_string()),
                        ContentItemKind("user.image".to_string()),
                        ContentItemKind("user.image".to_string()),
                        ContentItemKind("user.image".to_string()),
                    ]),
                    ..Default::default()
                },
            ),
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call-image".to_string()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::File {
                        file_id: "file_tool".to_string(),
                    },
                    detail: Some(ImageDetail::High),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,%%%".to_string(),
                    },
                    detail: Some(ImageDetail::High),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: large_image_url,
                    },
                    detail: Some(ImageDetail::High),
                },
            ]),
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    prepare_response_items(
        "image-preparation-thread",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Enabled,
        &InlineAttachmentStore,
    )
    .await;
    let expected_user_notice = concat!(
        "<image_resize_notice>\n",
        "Image 4 of 4 in the preceding user message was resized from 2048x2048 to 1600x1600 pixels.\n",
        "</image_resize_notice>"
    );

    let ResponseItem::Message {
        content,
        internal_chat_message_metadata_passthrough,
        ..
    } = &items[0]
    else {
        panic!("expected message");
    };
    assert_eq!(
        internal_chat_message_metadata_passthrough,
        &Some(InternalChatMessageMetadataPassthrough {
            content_item_kinds: Some(vec![
                ContentItemKind("user.image".to_string()),
                ContentItemKind("user.image".to_string()),
                ContentItemKind("images.preparation_error".to_string()),
                ContentItemKind("user.image".to_string()),
            ]),
            content_item_metadata: from_value(json!([{}, {}, {"harness_injected": true}, {}]))
                .unwrap(),
            ..Default::default()
        }),
    );
    let [
        ContentItem::InputImage {
            image: ImageReference::File { file_id },
            ..
        },
        ContentItem::InputImage {
            image:
                ImageReference::Inline {
                    image_url: small_message_image_url,
                },
            ..
        },
        ContentItem::InputText {
            text: failed_message_image,
        },
        ContentItem::InputImage {
            image:
                ImageReference::Inline {
                    image_url: resized_message_image_url,
                },
            ..
        },
    ] = content.as_slice()
    else {
        panic!("expected file image, unchanged image, failed image placeholder, and resized image");
    };
    assert_eq!(file_id, "file_message");
    assert_eq!(
        decoded_image(small_message_image_url).1.dimensions(),
        (64, 32)
    );
    assert_eq!(failed_message_image, IMAGE_PROCESSING_ERROR_PLACEHOLDER);
    assert_eq!(
        decoded_image(resized_message_image_url).1.dimensions(),
        (1600, 1600)
    );

    assert_eq!(
        &items[1],
        &ResponseItem::Message {
            status: None,
            encrypted_content: None,
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: expected_user_notice.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    content_item_kinds: Some(vec![ContentItemKind(
                        "images.resize_notice".to_string()
                    )]),
                    content_item_metadata: from_value(json!([{"harness_injected": true}])).unwrap(),
                    ..Default::default()
                },
            ),
        }
    );

    let ResponseItem::FunctionCallOutput { output, .. } = &items[2] else {
        panic!("expected function call output");
    };
    let [
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File { file_id },
            ..
        },
        FunctionCallOutputContentItem::InputText {
            text: failed_tool_image,
        },
        FunctionCallOutputContentItem::InputImage {
            image:
                ImageReference::Inline {
                    image_url: resized_tool_image_url,
                },
            ..
        },
    ] = output.content_items().expect("tool output content items")
    else {
        panic!("expected file image, failed image placeholder, and resized image in tool output");
    };
    assert_eq!(file_id, "file_tool");
    assert_eq!(failed_tool_image, IMAGE_PROCESSING_ERROR_PLACEHOLDER);
    assert_eq!(
        decoded_image(resized_tool_image_url).1.dimensions(),
        (1600, 1600)
    );
    assert_eq!(
        &items[3],
        &ResponseItem::Message {
            status: None,
            encrypted_content: None,
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: concat!(
                    "<image_resize_notice>\n",
                    "Image 3 of 3 in the preceding tool output was resized from 2048x2048 to 1600x1600 pixels.\n",
                    "</image_resize_notice>"
                )
                .to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: Some(
                InternalChatMessageMetadataPassthrough {
                    content_item_kinds: Some(vec![ContentItemKind(
                        "images.resize_notice".to_string()
                    )]),
                    content_item_metadata: from_value(json!([{"harness_injected": true}])).unwrap(),
                    ..Default::default()
                },
            ),
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn preparation_replaces_only_failed_tool_images_and_preserves_metadata() {
    let (valid_image_url, _) = png_data_url(/*width*/ 64, /*height*/ 32);
    let expected_valid_image_url = valid_image_url.clone();
    let mut items = vec![ResponseItem::CustomToolCallOutput {
        id: None,
        call_id: "call-1".to_string(),
        name: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::ContentItems(vec![
                FunctionCallOutputContentItem::InputText {
                    text: "before".to_string(),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,%%%".to_string(),
                    },
                    detail: Some(ImageDetail::High),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: data_url_from_bytes("image/png", b"not an image"),
                    },
                    detail: Some(ImageDetail::High),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: valid_image_url.clone(),
                    },
                    detail: Some(ImageDetail::Low),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: valid_image_url,
                    },
                    detail: Some(ImageDetail::High),
                },
            ]),
            success: Some(true),
        },
        internal_chat_message_metadata_passthrough: None,
    }];

    prepare_response_items(
        "image-preparation-thread",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &InlineAttachmentStore,
    )
    .await;

    assert_eq!(
        items,
        vec![ResponseItem::CustomToolCallOutput {
            id: None,
            call_id: "call-1".to_string(),
            name: None,
            output: FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: "before".to_string(),
                    },
                    FunctionCallOutputContentItem::InputText {
                        text: IMAGE_PROCESSING_ERROR_PLACEHOLDER.to_string(),
                    },
                    FunctionCallOutputContentItem::InputText {
                        text: IMAGE_PROCESSING_ERROR_PLACEHOLDER.to_string(),
                    },
                    FunctionCallOutputContentItem::InputText {
                        text: UNSUPPORTED_LOW_DETAIL_PLACEHOLDER.to_string(),
                    },
                    FunctionCallOutputContentItem::InputImage {
                        image: ImageReference::Inline {
                            image_url: expected_valid_image_url
                        },
                        detail: Some(ImageDetail::High),
                    },
                ]),
                success: Some(true),
            },
            internal_chat_message_metadata_passthrough: None,
        }]
    );
}

#[test]
fn preparation_errors_use_bounded_actionable_placeholders() {
    let cases = [
        (
            ImagePreparationError::RemoteUrlUnsupported,
            REMOTE_IMAGE_URL_PLACEHOLDER,
        ),
        (
            ImagePreparationError::UnsupportedLowDetail,
            UNSUPPORTED_LOW_DETAIL_PLACEHOLDER,
        ),
        (
            ImagePreparationError::Processing(ImageProcessingError::ImageTooLarge {
                representation: "decoded input",
                size: 2,
                max: 1,
            }),
            IMAGE_TOO_LARGE_PLACEHOLDER,
        ),
        (
            ImagePreparationError::Processing(ImageProcessingError::InvalidDataUrl {
                reason: "details remain in logs".to_string(),
            }),
            IMAGE_PROCESSING_ERROR_PLACEHOLDER,
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(error.placeholder(), expected);
    }
}

/// The submission-time backstop re-applies the per-result image limits to
/// tool outputs that never went through the producer's truncation (dynamic
/// tools, rehydrated rollouts): excess images become a bounded marker plus
/// empty slots, and the newest images survive.
#[tokio::test(flavor = "multi_thread")]
async fn tool_output_backstop_caps_unbounded_image_counts() {
    fn output_item(output: FunctionCallOutputPayload) -> ResponseItem {
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some("call_backstop".to_string()),
            name: None,
            namespace: None,
            output,
            internal_chat_message_metadata_passthrough: None,
        }
    }

    let total = codex_utils_output_truncation::MAX_TOOL_OUTPUT_IMAGES + 4;
    let payload = FunctionCallOutputPayload {
        body: FunctionCallOutputBody::ContentItems(
            (0..total)
                .map(|index| FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: format!("img-{index}"),
                    },
                    detail: None,
                })
                .chain(std::iter::once(FunctionCallOutputContentItem::InputText {
                    text: "caption".to_string(),
                }))
                .collect(),
        ),
        success: Some(true),
    };
    let mut items = vec![output_item(payload)];
    let _ = prepare_response_items(
        "thread-backstop",
        /*ephemeral*/ false,
        &mut items,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &InlineAttachmentStore,
    )
    .await;
    let ResponseItem::FunctionCallOutput { output, .. } = &items[0] else {
        panic!("payload item survived: {items:?}");
    };
    let prepared_items = output.content_items().expect("content items");
    let images: Vec<_> = prepared_items
        .iter()
        .filter(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
        .collect();
    assert_eq!(
        images.len(),
        5,
        "the backstop also enforces the shared visual-token ceiling"
    );
    let kept: Vec<_> = images
        .iter()
        .filter_map(|item| match item {
            FunctionCallOutputContentItem::InputImage {
                image: ImageReference::Inline { image_url },
                ..
            } => Some(image_url.clone()),
            _ => None,
        })
        .collect();
    let first_kept = total - 5;
    assert_eq!(
        kept,
        (first_kept..total)
            .map(|index| format!("img-{index}"))
            .collect::<Vec<_>>(),
        "the backstop keeps the newest images"
    );
    assert!(
        prepared_items.iter().any(|item| matches!(item,
            FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 7 image")
        )),
        "the backstop leaves a bounded marker: {prepared_items:?}"
    );

    // Already-bounded payloads pass through untouched: a second application
    // drops nothing and changes no item.
    let mut again = vec![output_item(output.clone())];
    let _ = prepare_response_items(
        "thread-backstop",
        /*ephemeral*/ false,
        &mut again,
        ImagePreparationMode::DetailBased,
        ImageResizeNoticeMode::Disabled,
        &InlineAttachmentStore,
    )
    .await;
    let ResponseItem::FunctionCallOutput { output: replay, .. } = &again[0] else {
        unreachable!()
    };
    assert_eq!(
        replay.content_items().expect("items"),
        output.content_items().expect("items"),
        "the backstop is idempotent"
    );
}
