use super::*;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;

use crate::context_manager::estimate_item_token_count;

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

fn tokens(items: &[ResponseItemEnvelope]) -> Vec<i64> {
    items
        .iter()
        .map(|envelope| estimate_item_token_count(&envelope.item))
        .collect()
}

/// Five evenly sized turns: each is large enough that the numbers stay readable.
fn five_turns() -> Vec<ResponseItemEnvelope> {
    (0..5)
        .map(|index| message(&format!("{index}").repeat(40_000)))
        .collect()
}

#[test]
fn the_replacement_starts_at_the_oldest_history() {
    let items = five_turns();
    let region = select_compaction_region(&items, 0, 0).expect("a region");
    assert_eq!(region.start, 0, "never skip older history");
    let balances = cut_balances(&items);
    assert_eq!(balances[region.end], 0, "the cut must be pairing-safe");
}

#[test]
fn a_verbatim_tail_is_retained() {
    let items = five_turns();
    let per_item = tokens(&items);
    let retain = per_item[0] * 2 + per_item[1] / 2;
    let region = select_compaction_region(&items, retain, 0).expect("a region");

    let retained: i64 = per_item[region.end..].iter().sum();
    assert!(
        retained >= retain,
        "retained {retained} tokens, expected at least {retain}"
    );
    assert_eq!(region.start, 0);
}

#[test]
fn the_span_is_capped_without_skipping_older_history() {
    let items = five_turns();
    let per_item = tokens(&items);
    let cap = per_item[0] + per_item[1] / 2;
    let region = select_compaction_region(&items, 0, cap).expect("a region");

    assert_eq!(region.start, 0, "the cap shortens the tail edge");
    assert!(
        region.tokens <= cap,
        "span {} tokens exceeds the cap {cap}",
        region.tokens
    );
}

#[test]
fn a_cut_never_lands_inside_a_tool_pair() {
    let items = vec![
        message(&"a".repeat(40_000)),
        call("call_1"),
        output("call_1", &"b".repeat(40_000)),
        message(&"c".repeat(40_000)),
    ];
    let per_item = tokens(&items);
    // Cap exactly at the end of the tool result: the next item would overflow.
    let cap: i64 = per_item[0..3].iter().sum();
    let region = select_compaction_region(&items, 0, cap).expect("a region");

    let balances = cut_balances(&items);
    assert_eq!(balances[region.end], 0, "cut split a tool pair");
    assert_eq!(region.end, 3, "the cut lands after the pair");
}

#[test]
fn nothing_to_replace_when_the_surface_is_tiny() {
    assert_eq!(select_compaction_region(&[message("only turn")], 1_000, 0), None);
    assert_eq!(select_compaction_region(&[], 0, 0), None);
}
