//! Unit coverage for the bounded streaming cap-partial capture.

use super::*;
use codex_protocol::ResponseItemId;
use pretty_assertions::assert_eq;

fn finish(capture: CapPartialCapture) -> CapPartialEvent {
    capture
        .finish("turn-1", "sampling:turn-1:0", None)
        .expect("captured fragments finish into an event")
}

#[test]
fn deltas_accumulate_into_one_fragment_per_key() {
    let mut capture = CapPartialCapture::default();
    for delta in ["par", "tial ", "cap ", "answer"] {
        capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", delta);
    }
    capture.observe_delta(CapPartialFragmentKind::Reasoning, "reasoning-1", "thinking");
    let event = finish(capture);
    assert_eq!(
        event
            .fragments
            .iter()
            .map(|fragment| (fragment.fragment_key.as_str(), fragment.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("item-1", "partial cap answer"),
            ("reasoning-1", "thinking")
        ]
    );
    assert_eq!(event.budget.total_bytes, 26);
    assert!(event.budget.enforced_caps.is_empty());
    assert_eq!(event.response_key, "sampling:turn-1:0");
    assert_eq!(event.turn_id, "turn-1");
    assert_eq!(event.reported_usage, None);
    assert_eq!(event.budget.token_evidence, TokenBudgetEvidence::Unverified);
}

#[test]
fn nothing_captured_finishes_none() {
    let capture = CapPartialCapture::default();
    assert!(capture.finish("t", "k", None).is_none());
}

#[test]
fn fragment_byte_ceiling_truncates_at_utf8_boundary_and_drops_excess() {
    let mut capture = CapPartialCapture::default();
    // ASCII filler then a 4-byte emoji: the cut lands inside the emoji and
    // backs off to its start.
    let mut first = "a".repeat(MAX_FRAGMENT_BYTES - 2);
    first.push('😀');
    first.push_str("tail");
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", &first);
    let event = finish(capture);
    let fragment = &event.fragments[0];
    assert!(fragment.truncated);
    assert_eq!(
        fragment.text.len(),
        MAX_FRAGMENT_BYTES - 2,
        "cut back to the emoji's start"
    );
    assert!(fragment.text.ends_with('a'));
    assert_eq!(
        event.budget.enforced_caps,
        vec![CapBudgetKind::FragmentBytes]
    );
    assert_eq!(event.budget.total_bytes as usize, MAX_FRAGMENT_BYTES - 2);

    // Once any bytes are dropped, later deltas cannot fill the spare UTF-8
    // bytes: that would splice a suffix onto the captured prefix.
    let mut capture = CapPartialCapture::default();
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", &first);
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", "late tail");
    let event = finish(capture);
    assert_eq!(event.fragments[0].text, "a".repeat(MAX_FRAGMENT_BYTES - 2));
    assert!(event.fragments[0].truncated);
    // A further delta to the saturated fragment drops without growing.
    let mut capture = CapPartialCapture::default();
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", &first);
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", "ab");
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-1", "more");
    // A different fragment still captures while the full one drops.
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-2", "other");
    let event = finish(capture);
    assert_eq!(event.fragments[0].text, "a".repeat(MAX_FRAGMENT_BYTES - 2));
    assert!(event.fragments[0].truncated);
    assert_eq!(event.fragments[1].text, "other");
}

#[test]
fn utf8_turn_budget_cut_closes_before_later_ascii_or_fragments() {
    let mut capture = CapPartialCapture::default();
    for index in 0..3 {
        capture.observe_delta(
            CapPartialFragmentKind::AssistantText,
            &format!("full-{index}"),
            &"x".repeat(MAX_FRAGMENT_BYTES),
        );
    }
    capture.observe_delta(
        CapPartialFragmentKind::AssistantText,
        "almost-full",
        &"x".repeat(MAX_FRAGMENT_BYTES - 2),
    );
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "last", "😀");
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "last", "ab");
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "late", "z");
    let event = finish(capture);
    assert_eq!(event.budget.total_bytes as usize, MAX_TURN_BYTES - 2);
    assert_eq!(event.fragments.len(), 5);
    assert_eq!(event.fragments[4].text, "");
    assert!(event.fragments[4].truncated);
    assert_eq!(event.budget.enforced_caps, vec![CapBudgetKind::TurnBytes]);
}

#[test]
fn turn_byte_ceiling_closes_the_capture() {
    let mut capture = CapPartialCapture::default();
    // Four fragments fill 64 KiB exactly; a fifth observes nothing.
    let chunk = "x".repeat(MAX_FRAGMENT_BYTES);
    for index in 0..4 {
        capture.observe_delta(
            CapPartialFragmentKind::AssistantText,
            &format!("item-{index}"),
            &chunk,
        );
    }
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "item-4", "overflow");
    let event = finish(capture);
    assert_eq!(event.fragments.len(), 4);
    assert_eq!(event.budget.total_bytes as usize, MAX_TURN_BYTES);
    assert!(
        !event.fragments.iter().any(|fragment| fragment.truncated),
        "the four stored fragments are complete prefixes"
    );
    assert_eq!(
        event.budget.enforced_caps,
        vec![CapBudgetKind::TurnBytes],
        "content observed past the ceiling records which budget dropped it"
    );
    // A partial fill that exceeds the ceiling truncates the last fragment.
    let mut capture = CapPartialCapture::default();
    for index in 0..3 {
        capture.observe_delta(
            CapPartialFragmentKind::AssistantText,
            &format!("item-{index}"),
            &chunk,
        );
    }
    capture.observe_delta(
        CapPartialFragmentKind::AssistantText,
        "item-3",
        &format!("{chunk}tail past the ceiling"),
    );
    let event = finish(capture);
    assert_eq!(event.fragments.len(), 4);
    assert_eq!(event.fragments[3].text.len(), MAX_FRAGMENT_BYTES);
    assert!(event.fragments[3].truncated);
    // Both budgets bound this cut: the fragment hit its ceiling and the
    // turn closed at its limit.
    assert_eq!(
        event.budget.enforced_caps,
        vec![CapBudgetKind::FragmentBytes, CapBudgetKind::TurnBytes]
    );
}

#[test]
fn fragment_count_ceiling_drops_further_fragments() {
    let mut capture = CapPartialCapture::default();
    for index in 0..MAX_FRAGMENTS {
        capture.observe_delta(
            CapPartialFragmentKind::AssistantText,
            &format!("item-{index}"),
            "x",
        );
    }
    capture.observe_delta(
        CapPartialFragmentKind::AssistantText,
        "item-late",
        "never stored",
    );
    let event = finish(capture);
    assert_eq!(event.fragments.len(), MAX_FRAGMENTS);
    assert_eq!(
        event.budget.enforced_caps,
        vec![CapBudgetKind::TurnFragments]
    );
    assert!(!event.fragments.iter().any(|f| f.text == "never stored"));
}

#[test]
fn whole_items_without_deltas_capture_once_and_not_twice() {
    let mut capture = CapPartialCapture::default();
    capture.observe_delta(CapPartialFragmentKind::AssistantText, "msg-1", "streamed ");
    // The same item completing must not append its full text again.
    capture.observe_item_done(&ResponseItem::Message {
        id: Some(ResponseItemId::from_server("msg-1".to_string())),
        role: "assistant".to_string(),
        content: vec![codex_protocol::models::ContentItem::OutputText {
            text: "streamed fully".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    // An item that never streamed captures whole.
    capture.observe_item_done(&ResponseItem::Message {
        id: Some(ResponseItemId::from_server("msg-2".to_string())),
        role: "assistant".to_string(),
        content: vec![codex_protocol::models::ContentItem::OutputText {
            text: "arrived whole".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    // Non-assistant items are ignored.
    capture.observe_item_done(&ResponseItem::Message {
        id: Some(ResponseItemId::from_server("user-1".to_string())),
        role: "user".to_string(),
        content: vec![codex_protocol::models::ContentItem::OutputText {
            text: "user text".to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    });
    let event = finish(capture);
    assert_eq!(
        event
            .fragments
            .iter()
            .map(|fragment| (fragment.fragment_key.as_str(), fragment.text.as_str()))
            .collect::<Vec<_>>(),
        vec![("msg-1", "streamed "), ("msg-2", "arrived whole")]
    );
}

#[test]
fn tool_arguments_are_non_executable_diagnostics() {
    let mut capture = CapPartialCapture::default();
    capture.observe_delta(
        CapPartialFragmentKind::ToolArguments,
        "call-1",
        "{\"command\":[\"ech",
    );
    capture.observe_item_done(&ResponseItem::CustomToolCall {
        id: Some(ResponseItemId::from_server("fc-2".to_string())),
        status: None,
        call_id: "call-2".to_string(),
        name: "shell".to_string(),
        namespace: None,
        input: "{\"command\":[\"echo\",\"hi\"]}".to_string(),
        internal_chat_message_metadata_passthrough: None,
    });
    let event = finish(capture);
    assert_eq!(
        event
            .fragments
            .iter()
            .map(|fragment| (fragment.kind, fragment.fragment_key.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (CapPartialFragmentKind::ToolArguments, "call-1"),
            (CapPartialFragmentKind::ToolArguments, "call-2"),
        ]
    );
    assert_eq!(event.fragments[0].text, "{\"command\":[\"ech");
    assert_eq!(
        event.fragments[1].text,
        "call shell: {\"command\":[\"echo\",\"hi\"]}"
    );
}

#[test]
fn idless_whole_items_get_distinct_keys() {
    let mut capture = CapPartialCapture::default();
    for text in ["first whole", "second whole"] {
        capture.observe_item_done(&ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![codex_protocol::models::ContentItem::OutputText {
                text: text.to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        });
    }
    let event = finish(capture);
    assert_eq!(event.fragments.len(), 2);
    assert_ne!(
        event.fragments[0].fragment_key,
        event.fragments[1].fragment_key
    );
}
