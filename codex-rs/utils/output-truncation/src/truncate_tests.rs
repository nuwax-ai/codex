use crate::TruncationPolicy;
use crate::approx_token_count;
use crate::approx_tokens_from_byte_count_i64;
use crate::formatted_truncate_text;
use crate::formatted_truncate_text_content_items_with_policy;
use crate::truncate_function_output_items_with_policy;
use crate::truncate_mcp_tool_result;
use crate::truncate_text;
use codex_protocol::mcp::CallToolResult;
use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;

#[test]
fn truncate_mcp_tool_result_borrows_small_result() {
    let result = CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": "hello",
        })],
        structured_content: Some(serde_json::json!({"x": 1})),
        is_error: Some(false),
        meta: Some(serde_json::json!({"k": "v"})),
    };

    let truncated = truncate_mcp_tool_result(&result, /*max_bytes*/ 1024);

    assert!(matches!(truncated, std::borrow::Cow::Borrowed(_)));
    assert_eq!(truncated.as_ref(), &result);
}

#[test]
fn truncate_mcp_tool_result_compacts_large_result() {
    let max_bytes = 1024;
    let result = CallToolResult {
        content: vec![serde_json::json!({
            "type": "text",
            "text": format!("head\n{}\ntail", "\"\\\n".repeat(5_000)),
        })],
        structured_content: Some(serde_json::json!({"x": "y".repeat(5_000)})),
        is_error: Some(true),
        meta: Some(serde_json::json!({
            "private": format!("meta-head{}meta-tail", "z".repeat(5_000)),
        })),
    };

    let truncated = truncate_mcp_tool_result(&result, max_bytes).into_owned();

    let preview = truncated.content[0]["text"]
        .as_str()
        .expect("truncated result should contain a text preview");
    assert_eq!(
        truncated,
        CallToolResult {
            content: vec![serde_json::json!({
                "type": "text",
                "text": preview,
            })],
            structured_content: None,
            is_error: Some(true),
            meta: None,
        }
    );
    assert!(serde_json::to_string(&truncated).unwrap().len() <= max_bytes);
    assert!(preview.contains("head"));
    assert!(preview.contains("chars truncated"));
    assert!(preview.contains("meta-tail"));

    let truncated_again = truncate_mcp_tool_result(&truncated, max_bytes);
    assert!(matches!(truncated_again, std::borrow::Cow::Borrowed(_)));
    assert_eq!(truncated_again.as_ref(), &truncated);
}

#[test]
fn truncate_bytes_less_than_placeholder_returns_placeholder() {
    let content = "example output";

    assert_eq!(
        "Warning: truncated output (original token count: 4)\nTotal output lines: 1\n\n…13 chars truncated…t",
        formatted_truncate_text(content, TruncationPolicy::Bytes(1)),
    );
}

#[test]
fn truncate_tokens_less_than_placeholder_returns_placeholder() {
    let content = "example output";

    assert_eq!(
        "Warning: truncated output (original token count: 4)\nTotal output lines: 1\n\nex…3 tokens truncated…ut",
        formatted_truncate_text(content, TruncationPolicy::Tokens(1)),
    );
}

#[test]
fn truncate_tokens_under_limit_returns_original() {
    let content = "example output";

    assert_eq!(
        content,
        formatted_truncate_text(content, TruncationPolicy::Tokens(10)),
    );
}

#[test]
fn truncate_bytes_under_limit_returns_original() {
    let content = "example output";

    assert_eq!(
        content,
        formatted_truncate_text(content, TruncationPolicy::Bytes(20)),
    );
}

#[test]
fn truncate_tokens_over_limit_returns_truncated() {
    let content = "this is an example of a long output that should be truncated";

    assert_eq!(
        "Warning: truncated output (original token count: 15)\nTotal output lines: 1\n\nthis is an…10 tokens truncated… truncated",
        formatted_truncate_text(content, TruncationPolicy::Tokens(5)),
    );
}

#[test]
fn truncate_bytes_over_limit_returns_truncated() {
    let content = "this is an example of a long output that should be truncated";

    assert_eq!(
        "Warning: truncated output (original token count: 15)\nTotal output lines: 1\n\nthis is an exam…30 chars truncated…ld be truncated",
        formatted_truncate_text(content, TruncationPolicy::Bytes(30)),
    );
}

#[test]
fn truncate_bytes_reports_original_line_count_when_truncated() {
    let content =
        "this is an example of a long output that should be truncated\nalso some other line";

    assert_eq!(
        "Warning: truncated output (original token count: 21)\nTotal output lines: 2\n\nthis is an exam…51 chars truncated…some other line",
        formatted_truncate_text(content, TruncationPolicy::Bytes(30)),
    );
}

#[test]
fn truncate_tokens_reports_original_line_count_when_truncated() {
    let content =
        "this is an example of a long output that should be truncated\nalso some other line";

    assert_eq!(
        "Warning: truncated output (original token count: 21)\nTotal output lines: 2\n\nthis is an example o…11 tokens truncated…also some other line",
        formatted_truncate_text(content, TruncationPolicy::Tokens(10)),
    );
}

#[test]
fn truncate_middle_bytes_handles_utf8_content() {
    let s = "😀😀😀😀😀😀😀😀😀😀\nsecond line with text\n";
    let out = truncate_text(s, TruncationPolicy::Bytes(20));
    assert_eq!(out, "😀😀…21 chars truncated…with text\n");
}

#[test]
fn truncates_across_multiple_under_limit_texts_and_reports_omitted() {
    let chunk = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega.\n";
    let chunk_tokens = approx_token_count(chunk);
    assert!(chunk_tokens > 0, "chunk must consume tokens");
    // The image in the middle now reserves its visual cost from the same
    // budget; size the policy so the text flow under test is unchanged.
    let limit = crate::TOOL_OUTPUT_IMAGE_VISUAL_TOKENS + chunk_tokens * 3;
    let t1 = chunk.to_string();
    let t2 = chunk.to_string();
    let t3 = chunk.repeat(10);
    let t4 = chunk.to_string();
    let t5 = chunk.to_string();

    let items = vec![
        FunctionCallOutputContentItem::InputText { text: t1.clone() },
        FunctionCallOutputContentItem::InputText { text: t2.clone() },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:mid".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputText { text: t3 },
        FunctionCallOutputContentItem::InputText { text: t4 },
        FunctionCallOutputContentItem::InputText { text: t5 },
    ];

    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(limit), |_| 0);

    assert_eq!(output.len(), 5);

    let first_text = match &output[0] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected first item: {other:?}"),
    };
    assert_eq!(first_text, &t1);

    let second_text = match &output[1] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected second item: {other:?}"),
    };
    assert_eq!(second_text, &t2);

    assert_eq!(
        output[2],
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:mid".to_string()
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        }
    );

    let fourth_text = match &output[3] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected fourth item: {other:?}"),
    };
    assert!(
        fourth_text.contains("tokens truncated"),
        "expected marker in truncated snippet: {fourth_text}"
    );

    let summary_text = match &output[4] {
        FunctionCallOutputContentItem::InputText { text } => text,
        other => panic!("unexpected summary item: {other:?}"),
    };
    assert!(summary_text.contains("omitted 2 text items"));
}

#[test]
fn truncate_function_output_items_with_policy_discards_empty_text() {
    let mut items = vec![
        FunctionCallOutputContentItem::InputText {
            text: String::new(),
        };
        16_384
    ];
    for policy in [TruncationPolicy::Bytes(0), TruncationPolicy::Tokens(1)] {
        assert_eq!(
            truncate_function_output_items_with_policy(&items, policy, |_| 0),
            Vec::new()
        );
    }

    let content = vec![
        FunctionCallOutputContentItem::InputText {
            text: "caption".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:one".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file_123".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputAudio {
            audio_url: "audio:one".to_string(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "enc_opaque".to_string(),
        },
    ];
    items.extend(content);
    // An unaffordable image becomes an explicit omission; preserving it
    // would violate the caller's shared visual/text budget.
    for policy in [TruncationPolicy::Bytes(16), TruncationPolicy::Tokens(4)] {
        let output = truncate_function_output_items_with_policy(&items, policy, |_| 1);
        assert!(
            !output
                .iter()
                .any(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. })),
            "{policy:?}: {output:?}"
        );
        assert!(output.iter().any(|item| matches!(
            item,
            FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 2 image")
        )));
        assert!(output.iter().any(|item| matches!(
            item,
            FunctionCallOutputContentItem::EncryptedContent { encrypted_content } if encrypted_content == "enc_opaque"
        )));
    }
}

#[test]
fn formatted_truncate_text_content_items_with_policy_returns_original_under_limit() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: "alpha".to_string(),
        },
        FunctionCallOutputContentItem::InputText {
            text: String::new(),
        },
        FunctionCallOutputContentItem::InputText {
            text: "beta".to_string(),
        },
    ];

    let (output, original_token_count) =
        formatted_truncate_text_content_items_with_policy(&items, TruncationPolicy::Bytes(32));

    assert_eq!(output, items);
    assert_eq!(original_token_count, None);
}

#[test]
fn formatted_truncate_text_content_items_with_policy_preserves_empty_leading_text_behavior() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: String::new(),
        },
        FunctionCallOutputContentItem::InputText {
            text: "abc".to_string(),
        },
    ];

    let (output, original_token_count) =
        formatted_truncate_text_content_items_with_policy(&items, TruncationPolicy::Bytes(0));

    assert_eq!(
        output,
        vec![FunctionCallOutputContentItem::InputText {
            text: "Warning: truncated output (original token count: 1)\nTotal output lines: 1\n\n…3 chars truncated…".to_string(),
        }]
    );
    assert_eq!(original_token_count, Some(1));
}

#[test]
fn formatted_truncate_text_content_items_with_policy_merges_text_and_appends_media() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: "abcd".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:one".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputText {
            text: "efgh".to_string(),
        },
        FunctionCallOutputContentItem::InputAudio {
            audio_url: "audio:one".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file_123".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputText {
            text: "ijkl".to_string(),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "img:two".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
    ];

    let (output, original_token_count) =
        formatted_truncate_text_content_items_with_policy(&items, TruncationPolicy::Bytes(8));

    // None of the images fits an eight-byte visual allowance.
    assert_eq!(original_token_count, Some(4));
    assert!(
        !output
            .iter()
            .any(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
    );
    assert!(matches!(
        output.last(),
        Some(FunctionCallOutputContentItem::InputText { text }) if text.contains("omitted 3 image")
    ));
}

#[test]
fn formatted_truncate_text_content_items_with_policy_preserves_encrypted_content() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: "abcdefgh".to_string(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "enc_opaque".to_string(),
        },
    ];

    let (output, original_token_count) =
        formatted_truncate_text_content_items_with_policy(&items, TruncationPolicy::Bytes(2));

    assert_eq!(
        output,
        vec![
            FunctionCallOutputContentItem::InputText {
                text: "Warning: truncated output (original token count: 2)\nTotal output lines: 1\n\na…6 chars truncated…h".to_string(),
            },
            FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "enc_opaque".to_string(),
            },
        ]
    );
    assert_eq!(original_token_count, Some(2));
}

#[test]
fn truncate_function_output_items_with_policy_omits_audio_over_budget() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: "abcdefgh".to_string(),
        },
        FunctionCallOutputContentItem::EncryptedContent {
            encrypted_content: "enc_opaque".to_string(),
        },
        FunctionCallOutputContentItem::InputAudio {
            audio_url: "audio:one".to_string(),
        },
    ];

    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Bytes(2), |_| 1);

    assert_eq!(
        output,
        vec![
            FunctionCallOutputContentItem::InputText {
                text: "a…6 chars truncated…h".to_string(),
            },
            FunctionCallOutputContentItem::EncryptedContent {
                encrypted_content: "enc_opaque".to_string(),
            },
            FunctionCallOutputContentItem::InputText {
                text: "[omitted 1 audio items ...]".to_string(),
            },
        ]
    );
}

#[test]
fn truncate_function_output_items_with_policy_charges_audio_against_byte_budget() {
    let audio = FunctionCallOutputContentItem::InputAudio {
        audio_url: "audio:one".to_string(),
    };
    let items = vec![
        audio.clone(),
        FunctionCallOutputContentItem::InputText {
            text: "abcdefgh".to_string(),
        },
    ];

    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Bytes(5), |_| 1);

    assert_eq!(
        output,
        vec![
            audio,
            FunctionCallOutputContentItem::InputText {
                text: truncate_text("abcdefgh", TruncationPolicy::Bytes(1)),
            },
        ]
    );
}

#[test]
fn formatted_truncate_text_content_items_with_policy_merges_all_text_for_token_budget() {
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: "abcdefgh".to_string(),
        },
        FunctionCallOutputContentItem::InputText {
            text: "ijklmnop".to_string(),
        },
    ];

    let (output, original_token_count) =
        formatted_truncate_text_content_items_with_policy(&items, TruncationPolicy::Tokens(2));

    assert_eq!(
        output,
        vec![FunctionCallOutputContentItem::InputText {
            text: "Warning: truncated output (original token count: 5)\nTotal output lines: 2\n\nabcd…3 tokens truncated…mnop".to_string(),
        }]
    );
    assert_eq!(original_token_count, Some(5));
}

#[test]
fn byte_count_conversion_clamps_non_positive_values() {
    assert_eq!(approx_tokens_from_byte_count_i64(/*bytes*/ -1), 0);
    assert_eq!(approx_tokens_from_byte_count_i64(/*bytes*/ 0), 0);
    assert_eq!(approx_tokens_from_byte_count_i64(/*bytes*/ 5), 2);
}

// ---------------------------------------------------------------------------
// Per-tool-output image entrance limits
// ---------------------------------------------------------------------------

use crate::MAX_TOOL_OUTPUT_IMAGE_BYTES;
use crate::MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES;
use crate::MAX_TOOL_OUTPUT_IMAGES;
use crate::TOOL_OUTPUT_IMAGE_VISUAL_TOKENS;

fn image_item(
    name: &str,
    detail: Option<codex_protocol::models::ImageDetail>,
) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputImage {
        image: ImageReference::Inline {
            image_url: name.to_string(),
        },
        detail,
    }
}

fn text_item(text: &str) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputText {
        text: text.to_string(),
    }
}

fn kept_image_urls(output: &[FunctionCallOutputContentItem]) -> Vec<String> {
    output
        .iter()
        .filter_map(|item| match item {
            FunctionCallOutputContentItem::InputImage {
                image: ImageReference::Inline { image_url },
                ..
            } => Some(image_url.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn image_hard_token_cap_keeps_the_newest_images_and_reports_the_rest() {
    let items: Vec<_> = (0..MAX_TOOL_OUTPUT_IMAGES + 3)
        .map(|index| image_item(&format!("img-{index}"), Some(DEFAULT_IMAGE_DETAIL)))
        .collect();
    let output = truncate_function_output_items_with_policy(
        &items,
        TruncationPolicy::Tokens(TOOL_OUTPUT_IMAGE_VISUAL_TOKENS * MAX_TOOL_OUTPUT_IMAGES),
        |_| 0,
    );
    assert_eq!(
        kept_image_urls(&output),
        (6..MAX_TOOL_OUTPUT_IMAGES + 3)
            .map(|index| format!("img-{index}"))
            .collect::<Vec<_>>(),
        "the newest images survive the count cap"
    );
    assert!(output.iter().any(|item| matches!(item,
        FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 6 image")
    )));
}

#[test]
fn oversized_image_is_refused_before_decode_and_reported() {
    let big = format!(
        "data:image/png;base64,{}",
        "A".repeat(MAX_TOOL_OUTPUT_IMAGE_BYTES + 1)
    );
    let items = vec![
        image_item(&big, Some(DEFAULT_IMAGE_DETAIL)),
        image_item("img-small", Some(DEFAULT_IMAGE_DETAIL)),
        text_item("context"),
    ];
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(10_000), |_| 0);
    assert_eq!(kept_image_urls(&output), vec!["img-small".to_string()]);
    assert!(
        output.iter().any(|item| matches!(item,
        FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 1 image")
        )),
        "{output:?}"
    );
    assert!(!output.iter().any(|item| matches!(item,
        FunctionCallOutputContentItem::InputText { text } if text.contains("AAAA")
    )));
}

#[test]
fn aggregate_byte_cap_drops_oldest_images_first() {
    let chunk = MAX_TOOL_OUTPUT_IMAGE_TOTAL_BYTES / 3 + 1024;
    assert!(chunk < MAX_TOOL_OUTPUT_IMAGE_BYTES);
    let items: Vec<_> = (0..3)
        .map(|index| {
            image_item(
                &format!("data:image/png;base64,{},{}", index, "B".repeat(chunk)),
                Some(DEFAULT_IMAGE_DETAIL),
            )
        })
        .collect();
    let output = truncate_function_output_items_with_policy(
        &items,
        TruncationPolicy::Tokens(usize::MAX),
        |_| 0,
    );
    assert_eq!(kept_image_urls(&output).len(), 2, "{output:?}");
    let kept = kept_image_urls(&output);
    assert!(
        kept.iter()
            .all(|url| !url.starts_with("data:image/png;base64,0,"))
    );
    assert!(output.iter().any(|item| matches!(item,
    FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 1 image")
    )));
}

#[test]
fn text_and_images_share_one_budget() {
    // Text that alone fits the budget, plus images whose visual cost must
    // come out of the same budget: the images are admitted newest-first and
    // the text collapses into the remainder.
    let policy = TruncationPolicy::Tokens(10_000);
    let items = vec![
        text_item(&"word ".repeat(200)),
        image_item("img-a", Some(DEFAULT_IMAGE_DETAIL)),
        image_item("img-b", Some(DEFAULT_IMAGE_DETAIL)),
        image_item("img-c", Some(DEFAULT_IMAGE_DETAIL)),
    ];
    let output = truncate_function_output_items_with_policy(&items, policy, |_| 0);
    // 10_000 / 1_844 -> all three images fit by count and budget.
    assert_eq!(kept_image_urls(&output).len(), 3);
    let kept_text: usize = output
        .iter()
        .filter_map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => Some(approx_token_count(text)),
            _ => None,
        })
        .sum();
    assert!(
        kept_text <= 10_000 - 3 * TOOL_OUTPUT_IMAGE_VISUAL_TOKENS,
        "text may only spend what the images left: {kept_text}"
    );
}

#[test]
fn original_detail_uses_a_conservative_visual_charge() {
    let items = vec![image_item(
        "original",
        Some(codex_protocol::models::ImageDetail::Original),
    )];
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(10_000), |_| 0);
    assert_eq!(kept_image_urls(&output), Vec::<String>::new());
    assert!(output.iter().any(
        |item| matches!(item, FunctionCallOutputContentItem::InputText { text }
        if text.contains("unknown original costs 10000 tokens plus control headroom"))
    ));
}

#[test]
fn an_unaffordable_newest_image_is_reported_explicitly() {
    let items = vec![image_item("img-a", Some(DEFAULT_IMAGE_DETAIL))];
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(1), |_| 0);
    assert_eq!(kept_image_urls(&output), Vec::<String>::new());
    assert!(output.iter().any(|item| matches!(item, FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 1 image"))));
}

#[test]
fn byte_policies_charge_images_in_bytes() {
    let items = vec![
        text_item("words for the budget"),
        image_item("img-a", Some(DEFAULT_IMAGE_DETAIL)),
        image_item("img-b", Some(DEFAULT_IMAGE_DETAIL)),
    ];
    // ~10k bytes ≈ 2.5k tokens: one image fits, the second does not.
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Bytes(10_000), |_| 0);
    let kept = kept_image_urls(&output);
    assert_eq!(kept, vec!["img-b".to_string()], "{output:?}");
}

#[test]
fn file_references_pass_size_limits_but_still_cost_visual_tokens() {
    let items = vec![
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file_1".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "file_2".to_string(),
            },
            detail: Some(DEFAULT_IMAGE_DETAIL),
        },
        text_item("tail text"),
    ];
    let output = truncate_function_output_items_with_policy(
        &items,
        TruncationPolicy::Tokens(2 * TOOL_OUTPUT_IMAGE_VISUAL_TOKENS),
        |_| 0,
    );
    let kept = output
        .iter()
        .filter(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
        .count();
    assert_eq!(kept, 2);
    assert!(!output.iter().any(|item| matches!(item,
        FunctionCallOutputContentItem::InputText { text } if text.contains("image")
    )));
}
