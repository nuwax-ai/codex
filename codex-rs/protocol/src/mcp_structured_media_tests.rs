use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn structured_mcp_result_preserves_image_and_success() {
    for is_error in [None, Some(false), Some(true)] {
        let result = CallToolResult {
            content: vec![
                json!({"type":"text","text":"text fallback must not be duplicated"}),
                json!({"type":"image","data":"IMAGE","mimeType":"image/png", "_meta":{"codex/imageDetail":"original"}}),
            ],
            structured_content: Some(json!({"width":640,"height":480})),
            is_error,
            meta: Some(json!({"private":"must not enter model content"})),
        };
        assert_eq!(
            result.as_function_call_output_payload(),
            FunctionCallOutputPayload {
                body: FunctionCallOutputBody::ContentItems(vec![
                    FunctionCallOutputContentItem::InputText {
                        text: json!({"width":640,"height":480}).to_string(),
                    },
                    FunctionCallOutputContentItem::InputImage {
                        image: ImageReference::Inline {
                            image_url: "data:image/png;base64,IMAGE".into(),
                        },
                        detail: Some(ImageDetail::Original),
                    },
                ]),
                success: Some(is_error != Some(true)),
            },
        );
    }
}

#[test]
fn structured_mcp_result_preserves_media_order() {
    let result = CallToolResult {
        content: vec![
            json!({"type":"audio","data":"AUDIO","mimeType":"audio/wav"}),
            json!({"type":"text","text":"fallback transcript"}),
            json!({"type":"image","data":"IMAGE","mimeType":"image/png"}),
        ],
        structured_content: Some(json!({"transcript":"structured transcript"})),
        is_error: Some(false),
        meta: None,
    };
    assert_eq!(
        result.as_function_call_output_payload(),
        FunctionCallOutputPayload {
            body: FunctionCallOutputBody::ContentItems(vec![
                FunctionCallOutputContentItem::InputText {
                    text: json!({"transcript":"structured transcript"}).to_string(),
                },
                FunctionCallOutputContentItem::InputAudio {
                    audio_url: "data:audio/wav;base64,AUDIO".into(),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,IMAGE".into(),
                    },
                    detail: Some(DEFAULT_IMAGE_DETAIL),
                },
            ]),
            success: Some(true),
        },
    );
}

#[test]
fn structured_mcp_result_keeps_text_only_preference() {
    let result = CallToolResult {
        content: vec![json!({"type":"text","text":"fallback"})],
        structured_content: Some(json!({"answer":42})),
        is_error: Some(false),
        meta: None,
    };
    assert_eq!(
        result.as_function_call_output_payload(),
        FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(json!({"answer":42}).to_string()),
            success: Some(true),
        },
    );
}

#[test]
fn structured_mcp_result_keeps_encrypted_precedence() {
    let result = CallToolResult {
        content: vec![
            json!({"type":"text","text":"opaque","_meta":{"codex/encryptedContent":true}}),
            json!({"type":"image","data":"IMAGE","mimeType":"image/png"}),
        ],
        structured_content: Some(json!({"secret":"must not override encrypted output"})),
        is_error: Some(true),
        meta: Some(json!({"private":"must not enter model content"})),
    };
    assert_eq!(
        result.as_function_call_output_payload(),
        FunctionCallOutputPayload {
            body: FunctionCallOutputBody::ContentItems(vec![
                FunctionCallOutputContentItem::EncryptedContent {
                    encrypted_content: "opaque".into(),
                },
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline {
                        image_url: "data:image/png;base64,IMAGE".into(),
                    },
                    detail: Some(DEFAULT_IMAGE_DETAIL),
                },
            ]),
            success: Some(false),
        },
    );
}
