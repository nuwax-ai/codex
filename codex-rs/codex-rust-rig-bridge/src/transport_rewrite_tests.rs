//! Wire-shape tests for the Chat/Anthropic body rewrite, asserting the exact
//! serialized body the transport sends — no HTTP stack needed.

use crate::RigProtocol;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;

fn rewrite<'a>(
    protocol: RigProtocol,
    tool_strict: &'a HashMap<String, bool>,
    tool_result_errors: &'a HashMap<String, bool>,
    anthropic_server_tools: &'a [Value],
    anthropic_tool_choice: Option<&'a Value>,
) -> super::ChatFamilyRewrite<'a> {
    super::ChatFamilyRewrite {
        protocol,
        disable_anthropic_parallel: false,
        disable_anthropic_thinking: false,
        tool_strict,
        tool_result_errors,
        anthropic_server_tools,
        anthropic_tool_choice,
        anthropic_effort: None,
        anthropic_service_tier: None,
        chat_drop_orphan_tool_choice: false,
    }
}

#[test]
fn hosted_only_anthropic_restores_tool_choice_with_parallel_flag() {
    let server_tool = json!({
        "type": "web_search_20250305",
        "name": "web_search",
        "allowed_domains": ["docs.rs"],
    });
    let server_tools = vec![server_tool.clone()];
    let choice = json!({"type": "auto"});
    let strict = HashMap::new();
    let result_errors = HashMap::new();
    let mut rewrite = rewrite(
        RigProtocol::Anthropic,
        &strict,
        &result_errors,
        &server_tools,
        Some(&choice),
    );
    rewrite.disable_anthropic_parallel = true;
    // rig's streaming path serialized neither `tools` nor `tool_choice`.
    let mut body = json!({"model": "m", "messages": []});
    assert!(rewrite.needed());
    rewrite.apply(&mut body);
    assert_eq!(
        body,
        json!({
            "model": "m",
            "messages": [],
            "tools": [server_tool],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
        })
    );
}

#[test]
fn restored_none_choice_is_never_parallel_disabled() {
    let server_tools = vec![json!({"type": "web_search_20250305", "name": "web_search"})];
    let choice = json!({"type": "none"});
    let strict = HashMap::new();
    let result_errors = HashMap::new();
    let mut rewrite = rewrite(
        RigProtocol::Anthropic,
        &strict,
        &result_errors,
        &server_tools,
        Some(&choice),
    );
    rewrite.disable_anthropic_parallel = true;
    let mut body = json!({"model": "m", "messages": []});
    rewrite.apply(&mut body);
    assert_eq!(
        body["tool_choice"],
        json!({"type": "none"}),
        "a restored `none` choice must not gain disable_parallel_tool_use"
    );
}

#[test]
fn serialized_rig_choice_is_not_overwritten_by_the_restore() {
    let server_tools = vec![json!({"type": "web_search_20250305", "name": "web_search"})];
    let choice = json!({"type": "auto"});
    let strict = HashMap::new();
    let result_errors = HashMap::new();
    let rewrite = rewrite(
        RigProtocol::Anthropic,
        &strict,
        &result_errors,
        &server_tools,
        Some(&choice),
    );
    // Mixed tools: rig serialized function tools plus its own tool_choice.
    let mut body = json!({
        "model": "m",
        "messages": [],
        "tools": [{"name": "shell", "input_schema": {}}],
        "tool_choice": {"type": "any"},
    });
    rewrite.apply(&mut body);
    assert_eq!(body["tool_choice"], json!({"type": "any"}));
    assert_eq!(body["tools"].as_array().map(Vec::len), Some(2));
}

#[test]
fn chat_dangling_tool_choice_is_dropped_when_no_tools_are_advertised() {
    let strict = HashMap::new();
    let result_errors = HashMap::new();
    let mut rewrite = rewrite(RigProtocol::Chat, &strict, &result_errors, &[], None);
    rewrite.chat_drop_orphan_tool_choice = true;
    assert!(rewrite.needed());
    let mut body = json!({
        "model": "m",
        "messages": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
    });
    rewrite.apply(&mut body);
    assert!(
        body.get("tool_choice").is_none(),
        "a dangling Chat tool_choice must be removed: {body}"
    );
}

#[test]
fn strict_flags_and_effort_merge_survive_the_extraction() {
    let strict = HashMap::from([("shell".to_string(), true)]);
    let result_errors = HashMap::new();
    let mut rewrite = rewrite(RigProtocol::Anthropic, &strict, &result_errors, &[], None);
    let effort = "low".to_string();
    rewrite.anthropic_effort = Some(&effort);
    let tier = "standard_only".to_string();
    rewrite.anthropic_service_tier = Some(&tier);
    let mut body = json!({
        "model": "m",
        "messages": [],
        "tools": [{"name": "shell", "input_schema": {}}],
        "output_config": {"format": {"type": "json_schema", "schema": {}}},
    });
    rewrite.apply(&mut body);
    assert_eq!(body["tools"][0]["strict"], json!(true));
    assert_eq!(body["output_config"]["effort"], json!("low"));
    assert_eq!(
        body["output_config"]["format"]["type"],
        json!("json_schema"),
        "the effort merge must keep rig's serialized output_config.format"
    );
    assert_eq!(body["service_tier"], json!("standard_only"));
}

#[test]
fn responses_passthrough_is_never_rewritten() {
    let strict = HashMap::new();
    let result_errors = HashMap::new();
    let mut rewrite = rewrite(RigProtocol::Responses, &strict, &result_errors, &[], None);
    rewrite.disable_anthropic_parallel = true;
    rewrite.chat_drop_orphan_tool_choice = true;
    assert!(!rewrite.needed());
}

#[tokio::test]
async fn wire_tee_caps_its_buffer_and_passes_every_chunk_through() {
    use bytes::Bytes;
    use futures::StreamExt;
    use std::sync::Arc;
    use std::sync::Mutex;

    let cap = super::WIRE_TEE_CAP_BYTES;
    let recorder = Arc::new(Mutex::new(Vec::<u8>::new()));
    // 12 MiB in 1 MiB chunks: well past the cap, all chunks must still flow.
    let chunk = Bytes::from(vec![b'x'; 1024 * 1024]);
    let source = futures::stream::iter((0..12).map(move |_| Ok(chunk.clone())));
    let body = Box::pin(source) as rig_core::http_client::sse::BoxedStream;
    let tee = super::tee_wire_bytes(body, Some(recorder.clone()));
    let passed: Vec<_> = tee.collect().await;
    assert_eq!(passed.len(), 12, "the cap must never drop stream chunks");
    assert!(
        passed
            .iter()
            .all(|chunk| chunk.as_ref().is_ok_and(|bytes| bytes.len() == 1024 * 1024)),
        "chunks pass through unchanged"
    );
    let captured = recorder.lock().unwrap().len();
    assert!(captured <= cap, "captured {captured} > cap {cap}");
    assert!(
        captured >= cap - 1024 * 1024,
        "capture should reach the cap"
    );
}
