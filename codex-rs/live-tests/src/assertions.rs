//! Protocol invariants asserted on real streams, shared by the
//! bridge-level suites, plus the cassette event classifier.

use super::*;

// ================================================================
// Assertions shared by the bridge-level suites
// ================================================================

pub fn assert_completed_with_usage(events: &[ResponseEvent], context: &str) {
    let completed = events
        .iter()
        .filter(|e| matches!(e, ResponseEvent::Completed { .. }))
        .count();
    assert_eq!(
        completed, 1,
        "{context}: expected exactly one Completed event"
    );
    let usage = events.iter().find_map(|e| match e {
        ResponseEvent::Completed {
            token_usage: Some(usage),
            ..
        } => Some(usage.clone()),
        _ => None,
    });
    let Some(usage) = usage else {
        panic!("{context}: Completed event should carry token usage");
    };
    // Plausibility invariants: a mis-mapped usage counter (e.g. swapped
    // input/output or a missing normalization) shows up here immediately.
    assert!(
        usage.input_tokens > 0,
        "{context}: input_tokens should be positive, got {usage:?}"
    );
    assert!(
        usage.total_tokens >= usage.input_tokens + usage.output_tokens,
        "{context}: total_tokens should cover input+output, got {usage:?}"
    );
}

pub fn text_len(events: &[ResponseEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            ResponseEvent::OutputTextDelta(delta) => delta.chars().count(),
            _ => 0,
        })
        .sum()
}

pub fn reasoning_len(events: &[ResponseEvent]) -> usize {
    events
        .iter()
        .map(|e| match e {
            ResponseEvent::ReasoningContentDelta { delta, .. } => delta.chars().count(),
            _ => 0,
        })
        .sum()
}

/// v0.17.4 event-ordering fix must hold on the wire: reasoning starts
/// streaming before message text, and the reasoning item completes first.
pub fn assert_reasoning_before_message(events: &[ResponseEvent], context: &str) {
    let position_of = |pred: &dyn Fn(&ResponseEvent) -> bool| events.iter().position(pred);
    if let (Some(r), Some(t)) = (
        position_of(&|e| matches!(e, ResponseEvent::ReasoningContentDelta { .. })),
        position_of(&|e| matches!(e, ResponseEvent::OutputTextDelta(_))),
    ) {
        assert!(
            r < t,
            "{context}: reasoning deltas must start before text deltas (got {r} vs {t})"
        );
    }
    if let (Some(r), Some(m)) = (
        position_of(&|e| {
            matches!(
                e,
                ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })
            )
        }),
        position_of(&|e| {
            matches!(
                e,
                ResponseEvent::OutputItemDone(ResponseItem::Message { .. })
            )
        }),
    ) {
        assert!(
            r < m,
            "{context}: reasoning item must complete before the message item (got {r} vs {m})"
        );
    }
}

/// Concatenated `ToolCallInputDelta`s must reassemble into exactly the final
/// `FunctionCall.arguments` string — validates delta computation on a real
/// stream.
/// Responses-wire variant: argument deltas are OPTIONAL on the wire (some
/// gateways deliver each call as one added+done pair without
/// `*.arguments.delta` frames). When deltas exist they must reassemble into
/// the final arguments; when none exist, every done FunctionCall must still
/// carry complete, parseable arguments.
pub fn assert_responses_tool_arguments_complete(events: &[ResponseEvent], context: &str) {
    use std::collections::HashMap;
    let mut deltas: HashMap<String, String> = HashMap::new();
    let mut final_args: HashMap<String, String> = HashMap::new();
    for event in events {
        match event {
            ResponseEvent::ToolCallInputDelta { item_id, delta, .. } => {
                *deltas.entry(item_id.clone()).or_default() += delta
            }
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id: Some(id),
                arguments,
                ..
            }) => {
                final_args.insert(id.to_string(), arguments.clone());
            }
            _ => {}
        }
    }
    if deltas.is_empty() {
        assert!(
            !final_args.is_empty(),
            "{context}: no argument deltas and no complete function call — nothing reassembles"
        );
        for (id, arguments) in &final_args {
            serde_json::from_str::<serde_json::Value>(arguments).unwrap_or_else(|error| {
                panic!("{context}: call {id} arguments are not valid JSON ({error}): {arguments}")
            });
        }
        return;
    }
    for (id, arguments) in &final_args {
        let assembled = deltas.get(id).map(String::as_str).unwrap_or("");
        assert_eq!(
            assembled, arguments,
            "{context}: concatenated deltas for {id} must equal the final arguments"
        );
    }
}

pub fn assert_tool_deltas_reassemble(events: &[ResponseEvent], context: &str) {
    let reassembled: String = events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::ToolCallInputDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    let final_args = events
        .iter()
        .find_map(|e| match e {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { arguments, .. }) => {
                Some(arguments.clone())
            }
            _ => None,
        })
        .unwrap_or_default();
    assert_eq!(
        reassembled, final_args,
        "{context}: concatenated tool-call deltas must equal the final arguments"
    );
}

pub fn end_turn_of(events: &[ResponseEvent]) -> Option<bool> {
    events.iter().find_map(|e| match e {
        ResponseEvent::Completed { end_turn, .. } => *end_turn,
        _ => None,
    })
}

// ================================================================
// Bridge-boundary cassette (record / replay)
// ================================================================

/// Stable kind tag per event, for A/B sequence diffs between bridges.
pub fn event_kind(event: &ResponseEvent) -> &'static str {
    match event {
        ResponseEvent::Created { .. } => "created",
        ResponseEvent::OutputItemAdded(item) => match item {
            ResponseItem::Message { .. } => "added.message",
            ResponseItem::Reasoning { .. } => "added.reasoning",
            ResponseItem::FunctionCall { .. } => "added.function_call",
            _ => "added.other",
        },
        ResponseEvent::OutputTextDelta(_) => "delta.text",
        ResponseEvent::ReasoningContentDelta { .. } => "delta.reasoning",
        ResponseEvent::ToolCallInputDelta { .. } => "delta.tool",
        ResponseEvent::OutputItemDone(item) => match item {
            ResponseItem::Message { .. } => "done.message",
            ResponseItem::Reasoning { .. } => "done.reasoning",
            ResponseItem::FunctionCall { .. } => "done.function_call",
            _ => "done.other",
        },
        ResponseEvent::Completed { .. } => "completed",
        _ => "other",
    }
}
