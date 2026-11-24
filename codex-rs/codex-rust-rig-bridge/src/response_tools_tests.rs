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
fn invalid_arguments_with_a_native_item_reach_the_tool_layer_instead_of_ending_the_turn() {
    // Fork (2026-11-24): this used to raise `Invalid Rig tool call: arguments
    // must be a JSON object`, which killed the whole turn. A native item means
    // the provider stated the call complete, so the model meant something:
    // publish the call and let the tool layer answer with a model-visible
    // error the model can retry from.
    let mut tools = pending();
    let mut call = cut_call();
    call.native = Some(rig_core::completion::message::Native {
        item: serde_json::json!({"type": "function_call", "call_id": "call_cut"}),
        fingerprint: rig_core::completion::message::Fingerprint::of(&serde_json::json!(
            {"type": "function_call", "call_id": "call_cut"}
        )),
        baseline: None,
        edited: false,
        blob_slot: None,
    });
    tools
        .complete("1".into(), call)
        .expect("a delivered call is published, not rejected");
    let events = tools.finish().expect("the turn continues");
    let published = events.get("1").expect("the call is published");
    assert!(
        published
            .iter()
            .any(|event| matches!(event, codex_api::ResponseEvent::OutputItemDone(_))),
        "the call is completed with its Done item: {published:?}"
    );
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

// ── Non-object arguments with a provider item (2026-11-24) ──────────────────
//
// A provider that sends arguments nothing can parse used to kill the whole
// turn here (`Invalid Rig tool call: arguments must be a JSON object`), which
// MiMo's chat wire produced intermittently. The call is now emitted instead:
// the tool layer turns bad arguments into a `RespondToModel` error, so the
// model retries inside the same turn.

fn native_item() -> rig_core::completion::message::Native {
    rig_core::completion::message::Native {
        item: serde_json::json!({"type": "function_call", "name": "x"}),
        fingerprint: rig_core::completion::message::Fingerprint::of(&"test-call"),
        baseline: None,
        edited: false,
        blob_slot: None,
    }
}

/// A call rig could not parse into an object, but which the provider did
/// state complete (`native` is present).
fn invalid_call(id: &str, tool: &str, raw: &str) -> ToolCall {
    let function = ToolFunction::parse(ToolName::new(tool).expect("non-empty"), raw);
    assert!(
        function.invalid_arguments.is_some(),
        "raw arguments {raw:?} must land in invalid_arguments"
    );
    let mut call = ToolCall::from_wire(id, function);
    call.native = Some(native_item());
    call
}

fn done_item(tools: &mut crate::response_tools::PendingTools, id: &str) -> codex_protocol::models::ResponseItem {
    let mut events = tools.finish().expect("the turn continues");
    let events = events.remove(id).expect("the call is published");
    events
        .into_iter()
        .find_map(|event| match event {
            codex_api::ResponseEvent::OutputItemDone(item) => Some(item),
            _ => None,
        })
        .expect("a completed call is published with its Done item")
}

#[test]
fn bare_patch_text_becomes_the_custom_tool_input() {
    // apply_patch is a custom tool: its input *is* the payload, so text the
    // provider sent outside JSON is exactly what the handler wants.
    let mut tools = crate::response_tools::PendingTools::new(Arc::new(HashSet::from([
        "apply_patch".to_string(),
    ])));
    let patch = "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch";
    tools
        .complete("1".into(), invalid_call("c1", "apply_patch", patch))
        .expect("a custom tool takes free-form input");
    match done_item(&mut tools, "1") {
        codex_protocol::models::ResponseItem::CustomToolCall { input, .. } => {
            assert_eq!(input, patch, "the provider's bytes are preserved");
        }
        other => panic!("expected a custom tool call, got {other:?}"),
    }
}

#[test]
fn truncated_object_arguments_reach_the_tool_layer_as_the_provider_sent_them() {
    // The output budget or a broken stream can leave a partial object; rig
    // still flags it, and the model should hear about the shape it produced.
    let mut tools = pending();
    tools
        .complete(
            "1".into(),
            invalid_call("c1", "shell", r#"{"command": ["echo""#),
        )
        .expect("a truncated call with a provider item is still a call");
    match done_item(&mut tools, "1") {
        codex_protocol::models::ResponseItem::FunctionCall { arguments, .. } => {
            assert_eq!(arguments, r#"{"command": ["echo""#);
        }
        other => panic!("expected a function call, got {other:?}"),
    }
}

#[test]
fn bare_text_for_a_function_tool_is_passed_through_verbatim() {
    // A function tool takes a JSON string. Passing the raw text through makes
    // core's parse fail into `RespondToModel`, which the model can act on —
    // strictly better than discarding the turn.
    let mut tools = pending();
    tools
        .complete("1".into(), invalid_call("c1", "shell", "echo hi please"))
        .expect("unparseable arguments are the tool layer's business");
    match done_item(&mut tools, "1") {
        codex_protocol::models::ResponseItem::FunctionCall { arguments, .. } => {
            assert_eq!(arguments, "echo hi please");
        }
        other => panic!("expected a function call, got {other:?}"),
    }
}

