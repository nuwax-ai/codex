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
    assert!(outcome.fits);
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
    assert!(
        !outcome.fits,
        "the caller must know the request is still over"
    );
    assert_eq!(outcome.total_bytes, before);
    assert_eq!(inline_image_bytes(&items), before);
}

fn resize_notice(body: &str) -> ResponseItem {
    ResponseItem::Message {
        encrypted_content: None,
        status: None,
        id: None,
        phase: None,
        role: "developer".to_string(),
        content: vec![ContentItem::InputText {
            text: format!("<image_resize_notice>\n{body}\n</image_resize_notice>"),
        }],
        internal_chat_message_metadata_passthrough: None,
    }
}

/// A second downscale must not leave the model with the dimensions from the
/// first one: an agent that maps screenshot coordinates through a stale notice
/// clicks the wrong place.
#[test]
fn a_second_downscale_corrects_the_resize_notice() {
    let mut items = vec![
        tool_output_image("call_a", "data:image/png;base64,AAAA"),
        resize_notice(
            "Image 1 of 1 in the preceding tool output was resized from 4320x16764 to 1546x6000 pixels.",
        ),
    ];
    {
        let mut refs: Vec<&mut ResponseItem> = items.iter_mut().collect();
        annotate_downscaled_notices(&mut refs, &[(0, 0, 1_024, 3_968)]);
    }
    let ResponseItem::Message { content, .. } = &items[1] else {
        panic!("expected the notice message");
    };
    let ContentItem::InputText { text } = &content[0] else {
        panic!("expected notice text");
    };
    assert!(
        text.contains("downscaled again to 1024x3968 pixels"),
        "notice must carry the corrected size: {text}"
    );
    assert!(text.contains("no longer apply"));
}

fn message_with_file_then_inline(file_id: &str, image_url: &str) -> ResponseItem {
    ResponseItem::Message {
        encrypted_content: None,
        status: None,
        id: None,
        phase: None,
        role: "user".to_string(),
        content: vec![
            ContentItem::InputImage {
                image: ImageReference::File {
                    file_id: file_id.to_string(),
                },
                detail: None,
            },
            ContentItem::InputImage {
                image: ImageReference::Inline {
                    image_url: image_url.to_string(),
                },
                detail: None,
            },
        ],
        internal_chat_message_metadata_passthrough: None,
    }
}

/// The notice numbers every image in the message, file-backed ones included.
/// The correction must use the same ordinal, or it points at the wrong image.
#[test]
fn the_correction_numbers_images_the_way_the_notice_does() {
    let mut items = vec![
        message_with_file_then_inline("file_1", "data:image/png;base64,AAAA"),
        resize_notice(
            "Image 2 of 2 in the preceding user message was resized from 1600x800 to 1200x600 pixels.",
        ),
    ];
    {
        let mut refs: Vec<&mut ResponseItem> = items.iter_mut().collect();
        // Slot 0 is the second image in the message: the first one is file-backed.
        annotate_downscaled_notices(&mut refs, &[(0, 0, 800, 400)]);
    }
    let ResponseItem::Message { content, .. } = &items[1] else {
        panic!("expected the notice message");
    };
    let ContentItem::InputText { text } = &content[0] else {
        panic!("expected notice text");
    };
    assert!(
        text.contains("Image 2 in the preceding message was downscaled again"),
        "correction must keep the notice's numbering: {text}"
    );
    assert!(
        !text.contains("Image 1 in the preceding message was downscaled"),
        "file-backed images occupy an ordinal too: {text}"
    );
}

/// Without a notice there is nothing to correct, and the pass must not invent
/// one.
#[test]
fn a_downscale_without_a_notice_adds_nothing() {
    let mut items = vec![tool_output_image("call_a", "data:image/png;base64,AAAA")];
    {
        let mut refs: Vec<&mut ResponseItem> = items.iter_mut().collect();
        annotate_downscaled_notices(&mut refs, &[(0, 0, 1_024, 3_968)]);
    }
    assert_eq!(items.len(), 1);
}
