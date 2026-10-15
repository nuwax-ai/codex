use super::*;
use codex_protocol::models::ImageDetail;
use codex_protocol::models::ImageReference;
use pretty_assertions::assert_eq;

const SMALL_ORIGINAL_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8DwH4QZYAwAR8oH+WdZbrcAAAAASUVORK5CYII=";
// A header-only fixture: dimensions are available, raster data is intentionally invalid.
const LARGE_ORIGINAL_PNG_HEADER: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAB9AAAAfQCAYAAACaOMR5AAAACElEQVQAAAAAAAAAAO5IXYcAAAAASUVORK5CYII=";

fn inline_original(url: &str) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputImage {
        image: ImageReference::Inline {
            image_url: url.to_string(),
        },
        detail: Some(ImageDetail::Original),
    }
}

fn image(index: usize, detail: ImageDetail) -> FunctionCallOutputContentItem {
    FunctionCallOutputContentItem::InputImage {
        image: ImageReference::File {
            file_id: format!("shot-{index}"),
        },
        detail: Some(detail),
    }
}

fn visible_tokens(items: &[FunctionCallOutputContentItem]) -> usize {
    items
        .iter()
        .map(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => approx_token_count(text),
            FunctionCallOutputContentItem::InputImage {
                image: ImageReference::Inline { image_url },
                detail: Some(ImageDetail::Original),
            } => codex_utils_image::estimate_original_image_tokens(image_url).unwrap_or(10_000),
            FunctionCallOutputContentItem::InputImage {
                detail: Some(ImageDetail::Original),
                ..
            } => 10_000,
            FunctionCallOutputContentItem::InputImage { .. } => 1_844,
            _ => panic!("unexpected media in budget regression"),
        })
        .sum()
}

#[test]
fn small_original_image_retains_its_detail_and_spends_its_patch_cost() {
    let items = vec![inline_original(SMALL_ORIGINAL_PNG)];
    let admission = admit_tool_output_images(&items, TruncationPolicy::Tokens(32));
    assert_eq!(admission.visual_tokens, 1);
    assert_eq!(
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(32), |_| 0),
        items
    );
}

#[test]
fn original_images_share_the_budget_and_unknown_images_are_not_free() {
    let items = vec![inline_original(LARGE_ORIGINAL_PNG_HEADER); 3];
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(10_000), |_| 0);
    assert_eq!(output[..2], items[1..]);
    assert_eq!(visible_tokens(&output[..2]), 7_938);
    assert!(visible_tokens(&output) <= 10_000);
    let unknown = vec![
        inline_original("data:image/png;base64,eA=="),
        image(0, ImageDetail::Original),
    ];
    let output = truncate_function_output_items_with_policy(
        &unknown,
        TruncationPolicy::Tokens(10_000),
        |_| 0,
    );
    assert_eq!(output.len(), 1);
    assert!(
        matches!(&output[0], FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 2 image"))
    );
}

#[test]
fn original_images_cannot_bypass_the_shared_output_budget() {
    let mut items: Vec<_> = (0..8)
        .map(|index| image(index, ImageDetail::Original))
        .collect();
    items.push(FunctionCallOutputContentItem::InputText {
        text: "x".repeat(40_000),
    });
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(10_000), |_| 0);
    assert!(visible_tokens(&output) <= 10_000, "{output:?}");
    assert!(
        !output
            .iter()
            .any(|item| matches!(item, FunctionCallOutputContentItem::InputImage { .. }))
    );
    assert!(output.iter().any(|item| matches!(item, FunctionCallOutputContentItem::InputText { text } if text.contains("original"))));
}

#[test]
fn large_caller_policy_cannot_exceed_the_single_output_hard_cap() {
    let mut items: Vec<_> = (0..8)
        .map(|index| image(index, ImageDetail::High))
        .collect();
    items.push(FunctionCallOutputContentItem::InputText {
        text: "x".repeat(80_000),
    });
    let output = truncate_function_output_items_with_policy(
        &items,
        TruncationPolicy::Tokens(usize::MAX),
        |_| 0,
    );
    assert!(
        visible_tokens(&output) <= 10_000,
        "{} tokens",
        visible_tokens(&output)
    );
}

#[test]
fn plain_text_and_structured_mcp_payloads_keep_the_same_hard_cap() {
    let structured = codex_protocol::mcp::CallToolResult {
        content: vec![],
        structured_content: Some(serde_json::json!({"data": "x".repeat(80_000)})),
        is_error: Some(false),
        meta: None,
    };
    let mut payloads = [
        FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text("x".repeat(80_000)),
            success: None,
        },
        structured.as_function_call_output_payload(),
    ];
    for payload in &mut payloads {
        truncate_function_output_payload(payload, TruncationPolicy::Tokens(usize::MAX), |_| 0);
        let FunctionCallOutputBody::Text(text) = &payload.body else {
            panic!("text payload");
        };
        assert!(approx_token_count(text) <= MAX_TOOL_OUTPUT_VISIBLE_TOKENS);
    }
}

#[test]
fn repeated_image_truncation_preserves_omission_and_wall_time() {
    let policy = TruncationPolicy::Tokens(1_844);
    let mut once = truncate_function_output_items_with_policy(
        &[image(0, ImageDetail::High), image(1, ImageDetail::High)],
        policy,
        |_| 0,
    );
    once.insert(
        0,
        FunctionCallOutputContentItem::InputText {
            text: "Wall time: 0.1000 seconds\nOutput:".to_string(),
        },
    );
    let twice = truncate_function_output_items_with_policy(&once, policy, |_| 0);
    assert_eq!(twice, once);
}

#[test]
fn running_code_mode_header_survives_but_similar_unbounded_text_is_charged() {
    let header = "Script running with cell ID cell-1\nWall time 0.1 seconds\nOutput:\n";
    let items = vec![
        FunctionCallOutputContentItem::InputText {
            text: header.to_string(),
        },
        FunctionCallOutputContentItem::InputText {
            text: format!("{header}{}", "x".repeat(40_000)),
        },
        image(0, ImageDetail::High),
    ];
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(1_844), |_| 0);
    assert_eq!(output.first(), items.first());
    assert!(visible_tokens(&output) < 2_000);
    assert!(!output.iter().any(|item| matches!(item, FunctionCallOutputContentItem::InputText { text } if text.len() > 1_000)));
}

#[test]
fn image_reference_and_data_url_metadata_have_independent_bounds() {
    let items = vec![
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::File {
                file_id: "x".repeat(MAX_IMAGE_REFERENCE_BYTES + 1),
            },
            detail: Some(ImageDetail::High),
        },
        FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: format!("data:{};base64,eA==", "x".repeat(MAX_IMAGE_REFERENCE_BYTES)),
            },
            detail: Some(ImageDetail::High),
        },
        image(2, ImageDetail::High),
    ];
    let output =
        truncate_function_output_items_with_policy(&items, TruncationPolicy::Tokens(10_000), |_| 0);
    assert_eq!(output.first(), items.last());
    assert_eq!(output.len(), 2);
    assert!(
        matches!(&output[1], FunctionCallOutputContentItem::InputText { text } if text.contains("omitted 2 image"))
    );
}
