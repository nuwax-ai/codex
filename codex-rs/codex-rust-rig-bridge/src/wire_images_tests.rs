use super::*;
use crate::RigProtocol;
use crate::wire_budget::WireBudget;
use codex_protocol::models::ResponseItem;
use serde_json::json;

const PREPARED_IMAGE_VISION_TOKENS: u64 = 1_844;

/// A 45-byte PNG whose IHDR declares 2560×2048: the bounded header estimate
/// prices it at 80×64 = 5120 patch tokens, while a converted prepared image
/// (Chat `high` / Anthropic detail-less) would only charge 1844.
const ORIGINAL_2560X2048: &str =
    "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAACgAAAAgACAYAAADqy2rxAAAAAElFTkSuQmCC";

fn item(value: serde_json::Value) -> ResponseItem {
    serde_json::from_value(value).expect("fixture item")
}

fn user_image_item(url: &str, detail: Option<&str>) -> ResponseItem {
    item(json!({
        "type":"message","role":"user","content":[
            {"type":"input_text","text":"see"},
            {"type":"input_image","image_url":url,"detail":detail}
        ]
    }))
}

fn tool_output_image_item(url: &str, detail: Option<&str>) -> ResponseItem {
    item(json!({
        "type":"function_call_output","call_id":"c1","output":[
            {"type":"input_text","text":"shot"},
            {"type":"input_image","image_url":url,"detail":detail}
        ]
    }))
}

#[test]
fn canonical_floor_prices_original_user_and_tool_images_by_patch_count() {
    let input = vec![
        user_image_item(ORIGINAL_2560X2048, Some("original")),
        item(json!({"type":"function_call","name":"view_image","call_id":"c1","arguments":"{}"})),
        tool_output_image_item(ORIGINAL_2560X2048, Some("original")),
    ];
    // Two retained originals: 2 × 5120 patch tokens.
    assert_eq!(canonical_vision_floor(&input), 2 * 5_120);
}

#[test]
fn canonical_floor_ignores_non_original_details_and_forged_text_data_urls() {
    let forged = data_url(&"QUJD".repeat(100));
    let input = vec![
        user_image_item(ORIGINAL_2560X2048, Some("high")),
        user_image_item(ORIGINAL_2560X2048, Some("auto")),
        user_image_item(ORIGINAL_2560X2048, None),
        tool_output_image_item(ORIGINAL_2560X2048, Some("low")),
        item(json!({"type":"message","role":"user","content":[
            {"type":"input_text","text":format!("look: {forged}")}
        ]})),
    ];
    // Every other detail already costs the prepared-image estimate on both
    // sides of the conversion, and a data URL inside text is not an image.
    assert_eq!(canonical_vision_floor(&input), 0);
}

#[test]
fn canonical_floor_charges_the_ceiling_for_originals_it_cannot_price() {
    let undecodable = data_url("QUJD"); // Valid Base64, not a PNG header.
    let input = vec![
        user_image_item(&undecodable, Some("original")),
        user_image_item("https://example.test/shot.png", Some("original")),
        item(json!({
            "type":"message","role":"user","content":[
                {"type":"input_image","file_id":"file-abc","detail":"original"}
            ]
        })),
    ];
    assert_eq!(
        canonical_vision_floor(&input),
        3 * ORIGINAL_DETAIL_PATCH_CEILING
    );
}

#[test]
fn canonical_floor_matches_the_passthrough_wire_estimate_for_originals() {
    // The floor must be exactly the canonical estimate the Responses walk
    // charges the same image on the passthrough wire — one pricing rule, so
    // the converting wires can never fall below it.
    let input = vec![user_image_item(ORIGINAL_2560X2048, Some("original"))];
    let walk = measure(
        RigProtocol::Responses,
        &json!({"input":[{"type":"message","role":"user","content":[
            {"type":"input_image","image_url":ORIGINAL_2560X2048,"detail":"original"}
        ]}]}),
    );
    assert_eq!(canonical_vision_floor(&input), walk.vision_tokens);
    assert_eq!(walk.vision_tokens, 5_120);
}

#[test]
fn canonical_floor_of_empty_and_compacted_shapes_is_zero() {
    assert_eq!(canonical_vision_floor(&[]), 0);
    // A compacted rebuild carries a summary message, not the dropped images.
    let compacted = vec![item(json!({
        "type":"message","role":"user","content":[
            {"type":"input_text","text":"Compaction summary: screenshots dropped."}
        ]
    }))];
    assert_eq!(canonical_vision_floor(&compacted), 0);
}

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
fn small_original_image_uses_header_patch_cost_on_both_detail_wires() {
    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=";
    for (protocol, body) in [
        (
            RigProtocol::Chat,
            json!({"messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": image_url, "detail": "original"}}]}]}),
        ),
        (
            RigProtocol::Responses,
            json!({"input": [{"type": "message", "role": "user", "content": [{"type": "input_image", "image_url": image_url, "detail": "original"}]}]}),
        ),
    ] {
        let costs = measure(protocol, &body);
        assert_eq!(
            costs,
            WireImageCosts {
                inline_payload_bytes: image_url.len(),
                vision_tokens: 1
            }
        );
        // The old flat 10K charge incorrectly rejected this legal tiny image.
        assert!(
            WireBudget {
                context_window_tokens: Some(4_000),
                output_tokens: Some(1_000),
                vision_floor_tokens: 0,
            }
            .check_body(serde_json::to_vec(&body).unwrap().len(), Some(costs))
            .is_ok()
        );
    }
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
        vision_floor_tokens: 0,
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
        vision_floor_tokens: 0,
    };
    assert!(
        reserved
            .check_body(framing + payload.len(), Some(image_costs))
            .is_err()
    );
}
