use super::*;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;

fn tool_output(id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item: ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(id.to_owned()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text(text.to_owned()),
            internal_chat_message_metadata_passthrough: None,
        },
        metadata: None,
    }
}

fn body_text(envelope: &ResponseItemEnvelope) -> &str {
    let ResponseItem::FunctionCallOutput { output, .. } = &envelope.item else {
        panic!("expected a tool output");
    };
    let FunctionCallOutputBody::Text(text) = &output.body else {
        panic!("expected text output");
    };
    text
}

#[test]
fn prunes_the_middle_and_keeps_head_and_tail() {
    let mut items = vec![tool_output("call_1", &"a".repeat(10_000))];
    let outcome = prune_oversized_tool_results(&mut items, 100, 200, 1_000);

    assert_eq!(outcome.pruned_results, 1);
    assert_eq!(outcome.chars_removed, 9_700);
    let text = body_text(&items[0]);
    assert!(text.starts_with(&"a".repeat(100)));
    assert!(text.ends_with(&"a".repeat(200)));
    let marker = "…<pruned 9700 chars>…";
    assert!(text.contains(marker));
    assert_eq!(text.chars().count(), 100 + 200 + marker.chars().count() + 2);
}

#[test]
fn leaves_small_outputs_untouched() {
    let original = "small output".to_string();
    let mut items = vec![tool_output("call_1", &original)];
    let outcome = prune_oversized_tool_results(&mut items, 10, 10, 1_000);

    assert!(outcome.is_empty());
    assert_eq!(body_text(&items[0]), original);
}

#[test]
fn never_splits_a_multibyte_character() {
    let mut items = vec![tool_output("call_1", &"中文内容".repeat(1_000))];
    let outcome = prune_oversized_tool_results(&mut items, 3, 3, 10);

    assert_eq!(outcome.pruned_results, 1);
    let text = body_text(&items[0]);
    // Three retained characters on each side, each intact.
    assert!(text.starts_with("中文内"));
    assert!(text.ends_with("文内容"));
}

#[test]
fn prunes_each_oversized_result_once_per_pass() {
    let mut items = vec![
        tool_output("call_1", &"x".repeat(5_000)),
        tool_output("call_2", "tiny"),
        tool_output("call_3", &"y".repeat(5_000)),
    ];
    let outcome = prune_oversized_tool_results(&mut items, 50, 50, 1_000);

    assert_eq!(outcome.pruned_results, 2);
    assert_eq!(body_text(&items[1]), "tiny");
    // A second pass finds nothing left to prune at the same threshold.
    let again = prune_oversized_tool_results(&mut items, 50, 50, 1_000);
    assert!(again.is_empty());
}

// ---- Fork (2026-11-10): stale-image pruning (FORK.md §5.5) ----

fn image_tool_output(id: &str, images: usize) -> ResponseItemEnvelope {
    let mut content = Vec::new();
    for _ in 0..images {
        content.push(FunctionCallOutputContentItem::InputImage {
            image: ImageReference::Inline {
                image_url: "data:image/png;base64,AAAA".to_owned(),
            },
            detail: None,
        });
    }
    ResponseItemEnvelope {
        item: ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(id.to_owned()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_content_items(content),
            internal_chat_message_metadata_passthrough: None,
        },
        metadata: None,
    }
}

fn image_count(envelope: &ResponseItemEnvelope) -> usize {
    let ResponseItem::FunctionCallOutput { output, .. } = &envelope.item else {
        panic!("expected a tool output");
    };
    let FunctionCallOutputBody::ContentItems(items) = &output.body else {
        panic!("expected content items");
    };
    items
        .iter()
        .filter(|item| is_inline_image(item))
        .count()
}

fn placeholder_count(envelope: &ResponseItemEnvelope) -> usize {
    let ResponseItem::FunctionCallOutput { output, .. } = &envelope.item else {
        panic!("expected a tool output");
    };
    let FunctionCallOutputBody::ContentItems(items) = &output.body else {
        panic!("expected content items");
    };
    items
        .iter()
        .filter(|item| matches!(item, FunctionCallOutputContentItem::InputText { text } if text == PRUNED_IMAGE_PLACEHOLDER))
        .count()
}

#[test]
fn keeps_the_newest_images_and_drops_older_ones() {
    let mut items = vec![
        image_tool_output("call_old", 3),
        image_tool_output("call_mid", 2),
        image_tool_output("call_new", 2),
    ];
    let pruned = prune_stale_images(&mut items, PROTECTED_RECENT_IMAGES);

    // 7 images total, newest 2 survive: 5 replaced, reported per call id.
    assert_eq!(
        pruned,
        vec![
            ("call_old".to_owned(), 3),
            ("call_mid".to_owned(), 2),
        ]
    );
    assert_eq!(image_count(&items[2]), 2, "the newest images survive");
    assert_eq!(placeholder_count(&items[0]), 3);
    assert_eq!(placeholder_count(&items[1]), 2);
}

#[test]
fn no_images_or_within_budget_is_a_noop() {
    let mut items = vec![tool_output("call_1", "plain text")];
    assert!(prune_stale_images(&mut items, PROTECTED_RECENT_IMAGES).is_empty());

    let mut items = vec![image_tool_output("call_1", 2)];
    assert!(prune_stale_images(&mut items, PROTECTED_RECENT_IMAGES).is_empty());
    assert_eq!(image_count(&items[0]), 2);
}

#[test]
fn replay_reapplies_the_same_image_replacement() {
    // Live: three images in one output, newest two protected.
    let mut live = vec![image_tool_output("call_1", 3)];
    let pruned = prune_stale_images(&mut live, PROTECTED_RECENT_IMAGES);
    assert_eq!(pruned, vec![("call_1".to_owned(), 1)]);
    assert_eq!(placeholder_count(&live[0]), 1);

    // Replay from the full rollout: the persisted count reproduces the same
    // surface — the oldest image becomes the placeholder again.
    let mut replayed = vec![image_tool_output("call_1", 3)];
    assert!(replay_pruned_images(&mut replayed, "call_1", 1));
    assert_eq!(placeholder_count(&replayed[0]), 1);
    assert_eq!(image_count(&replayed[0]), 2);

    // A missing call id is skipped, not an error.
    let mut other = vec![image_tool_output("call_2", 3)];
    assert!(!replay_pruned_images(&mut other, "call_1", 1));
    assert_eq!(image_count(&other[0]), 3);
}
