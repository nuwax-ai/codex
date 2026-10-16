//! Direct contracts for [`PendingTools`]' args-cut path: a tool call the
//! provider's own output budget closed without a native item is marked cut,
//! never executes, and can never resurrect at `finish` — the turn's real
//! terminal (usually the cap error, classified before `finish`) stays the
//! explanation. End-to-end coverage lives in codex-core's `rig_output_cap`
//! suite; these unit tests pin the state machine itself.

use std::collections::HashSet;
use std::sync::Arc;

use codex_api::ApiError;
use rig_core::completion::message::ToolCall;
use rig_core::completion::message::ToolFunction;
use rig_core::completion::message::ToolName;

fn pending() -> crate::response_tools::PendingTools {
    crate::response_tools::PendingTools::new(Arc::new(HashSet::new()))
}

fn name() -> ToolName {
    ToolName::new("shell").expect("non-empty tool name")
}

/// The shape rig produces when a provider's output budget cuts a Chat tool
/// call mid-arguments: the raw text moves to `invalid_arguments`, no native
/// provider item ever arrives.
fn cut_call() -> ToolCall {
    let function = ToolFunction::parse(name(), r#"{"command":["ech"#);
    assert!(
        function.invalid_arguments.is_some(),
        "truncated arguments must land in invalid_arguments"
    );
    ToolCall::from_wire("call_cut", function)
}

/// A fully valid provider call, as `End` would deliver after a healthy
/// stream.
fn healthy_call() -> ToolCall {
    ToolCall::from_wire(
        "call_healthy",
        ToolFunction::new(name(), serde_json::json!({"command": ["echo", "hi"]})),
    )
}

#[test]
fn cut_call_completes_without_error_and_fails_finish_with_the_cut_mark() {
    let mut tools = pending();
    tools
        .delta("1".into(), r#"{"command":["ech"#)
        .expect("argument fragments stream");
    // The cut close itself must NOT error: an error here would replace the
    // turn's real terminal (the cap) with a tool-shape complaint and make
    // core resample a budget failure.
    tools
        .complete("1".into(), cut_call())
        .expect("a cut close is recorded, not raised");
    match tools.finish() {
        Ok(events) => panic!(
            "a benign terminal must fail the turn instead of dropping the cut call, got {events:?}"
        ),
        Err(ApiError::Stream(message)) => assert!(
            message.contains("closed with incomplete arguments"),
            "the cut mark names the real cause, got: {message}"
        ),
        Err(other) => panic!("unexpected error class: {other:?}"),
    }
}

#[test]
fn cut_call_cannot_resurrect_at_final_even_after_a_late_valid_item() {
    let mut tools = pending();
    tools
        .delta("1".into(), r#"{"command":["ech"#)
        .expect("argument fragments stream");
    tools
        .complete("1".into(), cut_call())
        .expect("the cut close is recorded");
    // Even a later, fully valid provider item for the same position cannot
    // turn the cut call executable: the turn must still fail at final.
    tools
        .complete("1".into(), healthy_call())
        .expect("the late item alone parses");
    assert!(
        tools.finish().is_err(),
        "the cut mark is sticky: no OutputItemDone may resurrect at final"
    );
}

#[test]
fn a_cut_call_fails_finish_even_alongside_a_healthy_tool() {
    let mut tools = pending();
    tools
        .complete("2".into(), healthy_call())
        .expect("the healthy tool completes");
    tools
        .complete("1".into(), cut_call())
        .expect("the cut close is recorded");
    // Fail fast: the healthy tool's events are withheld rather than
    // executing a turn whose other call was cut.
    match tools.finish() {
        Ok(events) => panic!("a cut sibling must fail the whole final, got {events:?}"),
        Err(ApiError::Stream(message)) => assert!(
            message.contains("closed with incomplete arguments"),
            "the cut sibling names the cause, got: {message}"
        ),
        Err(other) => panic!("unexpected error class: {other:?}"),
    }
}

#[test]
fn invalid_arguments_with_a_native_item_stay_a_tool_shape_error_not_a_cut() {
    let mut tools = pending();
    let mut call = cut_call();
    call.native = Some(rig_core::completion::message::Native {
        item: serde_json::json!({"type": "function_call", "call_id": "call_cut"}),
        fingerprint: rig_core::completion::message::Fingerprint::of(&serde_json::json!(
            {"type": "function_call", "call_id": "call_cut"}
        )),
    });
    // A provider that DELIVERED an item with unparseable arguments is a
    // tool-shape complaint, not a budget cut: it must raise immediately.
    match tools.complete("1".into(), call) {
        Ok(()) => panic!("a native item with invalid arguments must error, not cut"),
        Err(ApiError::Stream(message)) => assert!(
            message.contains("arguments must be a JSON object"),
            "the native-item path keeps its own error, got: {message}"
        ),
        Err(other) => panic!("unexpected error class: {other:?}"),
    }
}

#[test]
fn cut_mark_survives_without_any_argument_fragments() {
    // A budget can cut a call after only its name streamed (the part opened,
    // no fragment ever arrived): the close still carries invalid_arguments
    // and no native item, and must mark cut the same way.
    let mut tools = pending();
    tools
        .complete("1".into(), cut_call())
        .expect("a fragmentless cut close is recorded");
    assert!(
        tools.finish().is_err(),
        "no fragments does not soften the cut contract"
    );
}
