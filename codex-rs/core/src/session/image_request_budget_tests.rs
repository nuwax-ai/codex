//! Coverage for the request-level image budget.
//!
//! These tests stay deliberately cheap: the re-encoding ladder is exercised by
//! `codex-utils-output-truncation` and `codex-utils-image`, and building real
//! images here cost minutes of CPU in a debug build for no extra coverage.

use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageDetail;
use pretty_assertions::assert_eq;

fn text_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        encrypted_content: None,
        status: None,
        id: None,
        phase: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        internal_chat_message_metadata_passthrough: None,
    }
}

fn tool_output_image(call_id: &str, image_url: &str) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id.to_owned()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload::from_content_items(vec![
            FunctionCallOutputContentItem::InputImage {
                image: ImageReference::Inline {
                    image_url: image_url.to_owned(),
                },
                detail: None,
            },
        ]),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn user_message_image(image_url: &str) -> ResponseItem {
    ResponseItem::Message {
        encrypted_content: None,
        status: None,
        id: None,
        phase: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: image_url.to_owned(),
            },
            detail: Some(ImageDetail::Auto),
        }],
        internal_chat_message_metadata_passthrough: None,
    }
}

fn inline_image_bytes(items: &[ResponseItem]) -> usize {
    let mut total = 0;
    for item in items {
        let mut slot = 0;
        while let Some(url) = inline_image_ref(item, slot) {
            total += url.len();
            slot += 1;
        }
    }
    total
}

#[test]
fn a_request_without_images_is_untouched() {
    let mut items = vec![text_message("hello")];
    let outcome = fit_request_images_to_budget(&mut items, 1024);
    assert!(outcome.is_empty());
    assert_eq!(outcome.total_bytes, 0);
}

#[test]
fn a_request_already_under_budget_is_not_re_encoded() {
    // The pass must decide from the byte lengths alone; reaching the encoder
    // here would already be a bug, so the payloads are deliberately not
    // decodable and must survive untouched.
    let first = "data:image/png;base64,AAAA";
    let second = "data:image/png;base64,BBBB";
    let mut items = vec![
        tool_output_image("call_a", first),
        user_message_image(second),
    ];
    let outcome = fit_request_images_to_budget(&mut items, 4096);
    assert!(outcome.is_empty());
    assert_eq!(outcome.total_bytes, inline_image_bytes(&items));
    assert_eq!(inline_image_bytes(&items), first.len() + second.len());
}

#[test]
fn undecodable_payloads_are_left_alone_instead_of_looping() {
    // Over budget, but the ladder cannot decode either image: the pass must
    // give up on them (and on them only) without dropping anything.
    let first = format!("data:image/png;base64,{}", "A".repeat(2048));
    let second = format!("data:image/png;base64,{}", "B".repeat(2048));
    let mut items = vec![
        tool_output_image("call_a", &first),
        user_message_image(&second),
    ];
    let before = inline_image_bytes(&items);
    let outcome = fit_request_images_to_budget(&mut items, 1024);
    assert!(outcome.is_empty(), "nothing could be re-encoded");
    assert_eq!(outcome.total_bytes, before);
    assert_eq!(inline_image_bytes(&items), before);
}
