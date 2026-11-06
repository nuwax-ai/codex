use super::*;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;

fn tool_output(id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item: ResponseItem::FunctionCallOutput {
            call_id: id.to_owned(),
            output: FunctionCallOutputPayload::from_text(text.to_owned()),
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
