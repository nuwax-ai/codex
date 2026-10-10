//! Unit coverage for the bounded cap-partial capture.

use super::*;

fn envelope(item: ResponseItem) -> ResponseItemEnvelope {
    ResponseItemEnvelope {
        item,
        metadata: None,
    }
}

fn assistant_message(id: &str, text: &str) -> ResponseItemEnvelope {
    envelope(ResponseItem::Message {
        id: Some(codex_protocol::ResponseItemId::from_server(id.to_string())),
        role: "assistant".to_string(),
        content: vec![codex_protocol::models::ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

fn user_message(text: &str) -> ResponseItemEnvelope {
    envelope(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![codex_protocol::models::ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

#[test]
fn captures_trailing_assistant_items_after_the_last_user_message() {
    let items = vec![
        user_message("earlier turn"),
        assistant_message("a1", "first answer"),
        user_message("this turn"),
        assistant_message("b1", "partial answer"),
        envelope(ResponseItem::Reasoning {
            id: Some(codex_protocol::ResponseItemId::from_server(
                "r1".to_string(),
            )),
            summary: vec![
                codex_protocol::models::ReasoningItemReasoningSummary::SummaryText {
                    text: "thinking prefix".to_string(),
                },
            ],
            content: None,
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        }),
    ];
    let event = build_cap_partial("turn-1", "turn-1", &items).expect("partial should capture");
    assert_eq!(event.turn_id, "turn-1");
    assert_eq!(event.fragments.len(), 2);
    assert_eq!(
        event.fragments[0].kind,
        CapPartialFragmentKind::AssistantText
    );
    assert_eq!(event.fragments[0].text, "partial answer");
    assert_eq!(event.fragments[1].kind, CapPartialFragmentKind::Reasoning);
    assert_eq!(event.fragments[1].text, "thinking prefix");
    assert!(event.budget.enforced_caps.is_empty());
    assert_eq!(event.budget.token_evidence.as_str(), "unverified");
}

#[test]
fn returns_none_when_nothing_assistant_preceded_the_cap() {
    let items = vec![user_message("this turn")];
    assert!(build_cap_partial("turn-1", "turn-1", &items).is_none());
}

#[test]
fn truncates_fragment_prefixes_at_utf8_boundaries() {
    // 3-byte CJK characters: cutting inside one would be invalid UTF-8.
    let long_text = "字".repeat(10_000);
    let items = vec![assistant_message("a1", &long_text)];
    let event = build_cap_partial("turn-1", "turn-1", &items).expect("partial should capture");
    let fragment = &event.fragments[0];
    assert!(fragment.truncated);
    assert!(fragment.text.len() <= 16 * 1024);
    assert!(fragment.text.chars().all(|c| c == '字'));
    assert!(
        event
            .budget
            .enforced_caps
            .contains(&CapBudgetKind::FragmentBytes)
    );
}

#[test]
fn stops_at_the_turn_byte_ceiling() {
    // Four fragments of ~10 KiB each stay under per-fragment limits but
    // together exceed the 64 KiB turn ceiling.
    let text = "x".repeat(10 * 1024);
    let items = vec![
        assistant_message("a1", &text),
        assistant_message("a2", &text),
        assistant_message("a3", &text),
        assistant_message("a4", &text),
        assistant_message("a5", &text),
        assistant_message("a6", &text),
        assistant_message("a7", &text),
    ];
    let event = build_cap_partial("turn-1", "turn-1", &items).expect("partial should capture");
    assert!(event.budget.total_bytes <= 64 * 1024);
    assert!(
        event
            .budget
            .enforced_caps
            .contains(&CapBudgetKind::TurnBytes)
    );
    // The seventh fragment arrives truncated at the remaining budget instead
    // of being dropped wholesale (prefix-keep).
    assert_eq!(event.fragments.len(), 7);
    assert!(event.fragments.last().expect("final fragment").truncated);
}

#[test]
fn stops_at_the_fragment_count_ceiling() {
    let items: Vec<_> = (0..40)
        .map(|index| assistant_message(&format!("a{index}"), "tiny"))
        .collect();
    let event = build_cap_partial("turn-1", "turn-1", &items).expect("partial should capture");
    assert_eq!(event.fragments.len(), 32);
    assert!(
        event
            .budget
            .enforced_caps
            .contains(&CapBudgetKind::TurnFragments)
    );
}

#[test]
fn records_tool_arguments_as_non_executable_diagnostics() {
    let items = vec![envelope(ResponseItem::FunctionCall {
        id: None,
        name: "write_file".to_string(),
        namespace: None,
        arguments: "{\"path\": \"/tmp/x\"}".to_string(),
        encrypted_function_args: None,
        call_id: "call-1".to_string(),
        internal_chat_message_metadata_passthrough: None,
    })];
    let event = build_cap_partial("turn-1", "turn-1", &items).expect("partial should capture");
    assert_eq!(
        event.fragments[0].kind,
        CapPartialFragmentKind::ToolArguments
    );
    assert!(event.fragments[0].text.contains("write_file"));
    assert!(event.fragments[0].text.contains("/tmp/x"));
    assert_eq!(event.fragments[0].item_id.as_deref(), Some("call-1"));
}

#[test]
fn trailing_items_span_back_to_the_last_user_message_only() {
    let items = vec![
        user_message("turn 1"),
        assistant_message("a1", "old"),
        user_message("turn 2"),
        assistant_message("a2", "new"),
    ];
    let trailing = trailing_turn_items(&items);
    assert_eq!(trailing.len(), 1);
}
