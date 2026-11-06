use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;

fn message(text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item: ResponseItem::Message {
            id: None,
            role: "user".to_owned(),
            content: vec![ContentItem::InputText {
                text: text.to_owned(),
            }],
            phase: None,
            status: None,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        },
        metadata: None,
    }
}

fn call(call_id: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item: ResponseItem::FunctionCall {
            encrypted_content: None,
            status: None,
            id: None,
            name: "shell".to_owned(),
            namespace: None,
            arguments: "{}".to_owned(),
            encrypted_function_args: None,
            call_id: call_id.to_owned(),
            internal_chat_message_metadata_passthrough: None,
        },
        metadata: None,
    }
}

fn output(call_id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item: ResponseItem::FunctionCallOutput {
            id: None,
            call_id: Some(call_id.to_owned()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text(text.to_owned()),
            internal_chat_message_metadata_passthrough: None,
        },
        metadata: None,
    }
}

/// A visible tool call plus its result must never be separated by the cut: the
/// model would receive an orphan result.
#[test]
fn the_cut_never_splits_a_tool_pair() {
    let items = vec![
        message(&"a".repeat(2_000)),
        call("call_1"),
        output("call_1", &"b".repeat(2_000)),
        message(&"c".repeat(2_000)),
    ];
    let region = select_compaction_region(&items, 0, 0).expect("a region");
    assert_eq!(region.end, 3, "the cut must land after the pair");
    assert_eq!(region.start, 0);
}

#[test]
fn a_verbatim_tail_is_retained() {
    let items = vec![
        message(&"a".repeat(4_000)),
        message(&"b".repeat(4_000)),
        message(&"c".repeat(4_000)),
    ];
    // Retain at least half the surface; only the oldest message may be replaced.
    let region = select_compaction_region(&items, 8_000, 0).expect("a region");
    assert_eq!(region.start, 0);
    assert_eq!(region.end, 1);
    assert!(region.tokens >= 900, "tokens: {}", region.tokens);
}

/// The span cap is what keeps a single summary request inside a small model's
/// window: the replacement shortens instead of exceeding the budget.
#[test]
fn the_span_is_capped_without_skipping_older_history() {
    let items = vec![
        message(&"a".repeat(4_000)),
        message(&"b".repeat(4_000)),
        message(&"c".repeat(4_000)),
    ];
    let region = select_compaction_region(&items, 0, 1_200).expect("a region");
    assert_eq!(region.start, 0, "never skip the oldest history");
    assert_eq!(region.end, 1);
    assert!(region.tokens <= 1_200, "tokens: {}", region.tokens);
}

#[test]
fn nothing_to_replace_when_everything_is_the_tail() {
    let items = vec![message("only turn")];
    assert_eq!(select_compaction_region(&items, 10_000, 0), None);
}

#[test]
fn a_leading_open_pair_pushes_the_cut_forward() {
    // The surface starts inside a pair (a resumed window can look like this);
    // the cap must not cut between the call and its result.
    let items = vec![
        call("call_1"),
        output("call_1", &"b".repeat(4_000)),
        message(&"c".repeat(4_000)),
    ];
    let region = select_compaction_region(&items, 0, 600).expect("a region");
    assert_eq!(region.end, 2, "the cut lands after the pair, never inside");
}
