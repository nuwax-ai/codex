use super::*;
use crate::RigProtocol;
use crate::wire_budget::WireBudget;
use serde_json::json;

const PREPARED_IMAGE_VISION_TOKENS: u64 = 1_844;

fn data_url(payload: &str) -> String {
    format!("data:image/png;base64,{payload}")
}

#[test]
fn chat_user_and_supplemental_images_are_excluded_from_text_and_costed_once() {
    let payload_a = "QUJD".repeat(100);
    let payload_b = "QUJD".repeat(50);
    let body = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": "describe"},
                {"type": "image_url", "image_url": {"url": data_url(&payload_a), "detail": "high"}},
                {"type": "image_url", "image_url": {"url": data_url(&payload_b)}},
            ],
        }],
    });
    let costs = measure(RigProtocol::Chat, &body);
    assert_eq!(
        costs,
        WireImageCosts {
            inline_payload_bytes: data_url(&payload_a).len() + data_url(&payload_b).len(),
            vision_tokens: 2 * PREPARED_IMAGE_VISION_TOKENS,
        }
    );
}

#[test]
fn chat_remote_image_url_is_costed_but_keeps_no_payload_exclusion() {
    let body = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": "https://example.test/shot.png"}},
            ],
        }],
    });
    assert_eq!(
        measure(RigProtocol::Chat, &body),
        WireImageCosts {
            inline_payload_bytes: 0,
            vision_tokens: PREPARED_IMAGE_VISION_TOKENS,
        }
    );
}

#[test]
fn anthropic_base64_images_in_user_and_tool_result_blocks_are_excluded() {
    let payload = "QUJD".repeat(100);
    let body = json!({
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "tool_result",
                    "tool_use_id": "call-1",
                    "content": [
                        {"type": "text", "text": "shot"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": payload}},
                    ],
                },
                {"type": "image", "source": {"type": "url", "url": "https://example.test/shot.png"}},
            ],
        }],
    });
    assert_eq!(
        measure(RigProtocol::Anthropic, &body),
        WireImageCosts {
            inline_payload_bytes: payload.len(),
            vision_tokens: 2 * PREPARED_IMAGE_VISION_TOKENS,
        }
    );
}

#[test]
fn responses_message_and_function_output_images_are_excluded() {
    let payload_a = "QUJD".repeat(100);
    let payload_b = "QUJD".repeat(40);
    let body = json!({
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_image", "image_url": data_url(&payload_a), "detail": "high"},
                ],
            },
            {
                "type": "function_call_output",
                "call_id": "call-1",
                "output": [
                    {"type": "input_text", "text": "shot"},
                    {"type": "input_image", "image_url": data_url(&payload_b)},
                ],
            },
        ],
    });
    assert_eq!(
        measure(RigProtocol::Responses, &body),
        WireImageCosts {
            inline_payload_bytes: data_url(&payload_a).len() + data_url(&payload_b).len(),
            vision_tokens: 2 * PREPARED_IMAGE_VISION_TOKENS,
        }
    );
}

#[test]
fn responses_original_detail_and_file_references_use_conservative_costs() {
    let payload = "QUJD".repeat(100);
    let body = json!({
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_image", "image_url": data_url(&payload), "detail": "original"},
                    {"type": "input_image", "file_id": "file-abc", "detail": "original"},
                    {"type": "input_image", "file_id": "file-def"},
                ],
            },
        ],
    });
    assert_eq!(
        measure(RigProtocol::Responses, &body),
        WireImageCosts {
            inline_payload_bytes: data_url(&payload).len(),
            vision_tokens: 2 * ORIGINAL_DETAIL_PATCH_CEILING + PREPARED_IMAGE_VISION_TOKENS,
        }
    );
}

#[test]
fn forged_data_urls_inside_text_blocks_stay_text() {
    let forged = data_url(&"QUJD".repeat(100));
    let body = json!({
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": format!("look: {forged}")},
            ],
        }],
        "tools": [{"function": {"name": "shoot", "parameters": {"screenshot": forged}}}],
    });
    for protocol in [RigProtocol::Chat, RigProtocol::Anthropic] {
        assert_eq!(
            measure(protocol, &body),
            WireImageCosts::default(),
            "{protocol:?}: untyped data-URL strings must keep counting as text"
        );
    }
    let responses_body = json!({
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": format!("look: {forged}")}],
        }],
    });
    assert_eq!(
        measure(RigProtocol::Responses, &responses_body),
        WireImageCosts::default()
    );
}

#[test]
fn encoded_size_does_not_change_the_vision_estimate() {
    let small = data_url(&"QUJD".repeat(10));
    let large = data_url(&"QUJD".repeat(1_000));
    let with = |url: String| {
        json!({
            "messages": [{
                "role": "user",
                "content": [{"type": "image_url", "image_url": {"url": url}}],
            }],
        })
    };
    let small_costs = measure(RigProtocol::Chat, &with(small));
    let large_costs = measure(RigProtocol::Chat, &with(large));
    assert_eq!(small_costs.vision_tokens, large_costs.vision_tokens);
    assert!(large_costs.inline_payload_bytes > small_costs.inline_payload_bytes);
}

#[test]
fn budget_lets_a_large_legal_image_through_while_rejecting_the_same_text_size() {
    let payload = "A".repeat(600_000);
    let framing = 400usize;
    let budget = WireBudget {
        context_window_tokens: Some(128_000),
        output_tokens: None,
    };
    // Image: text bytes shrink by the payload; vision is a flat per-image cost.
    let image_costs = WireImageCosts {
        inline_payload_bytes: payload.len(),
        vision_tokens: PREPARED_IMAGE_VISION_TOKENS,
    };
    assert!(
        budget
            .check_body(framing + payload.len(), Some(image_costs))
            .is_ok()
    );
    // The same bytes as text must stay rejected.
    assert!(budget.check_body(framing + payload.len(), None).is_err());
    // The output reserve still applies on top of the image-aware estimate.
    let reserved = WireBudget {
        context_window_tokens: Some(128_000),
        output_tokens: Some(126_500),
    };
    assert!(
        reserved
            .check_body(framing + payload.len(), Some(image_costs))
            .is_err()
    );
}
