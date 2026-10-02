use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn envelope(payload: serde_json::Value) -> serde_json::Value {
    // Same-source v1 envelope for these conversion tests; the gate itself is
    // covered by hosted_replay_tests.
    json!({"version": 1, "source": "source", "blocks": payload})
}

#[test]
fn hosted_replay_tracks_each_assistant_across_user_turns() {
    let assistant_items = [
        json!({"type":"function_call", "name":"lookup", "call_id":"call-1", "arguments":"{}"}),
        json!({"type":"custom_tool_call", "name":"custom", "call_id":"call-2", "input":"raw"}),
        json!({"type":"agent_message", "author":"worker", "recipient":"main", "content":[{"type":"input_text", "text":"result"}]}),
        json!({"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"answer"}]}),
    ];
    let expected_blocks = json!([
        {"type": "server_tool_use", "id": "search-2", "name": "web_search",
         "input": {"query": "new turn"}}
    ])
    .as_array()
    .expect("blocks")
    .clone();
    let blocks = envelope(json!(expected_blocks));
    for first in &assistant_items {
        // A search-only second turn must still get its own assistant message.
        for second in std::iter::once(None).chain(assistant_items.iter().map(Some)) {
            let mut input = vec![
                first.clone(),
                json!({"type":"message", "role":"user", "content":[{"type":"input_text", "text":"follow up"}]}),
            ];
            input.extend(second.cloned().map(|mut item| {
                if item.get("call_id").is_some() {
                    item["call_id"] = json!("second-call");
                }
                item
            }));
            input.push(json!({"type":"web_search_call", "wire_blocks":blocks}));
            let items: Vec<ResponseItem> = input
                .into_iter()
                .map(|item| serde_json::from_value(item).expect("history item"))
                .collect();
            let mut replay = Vec::new();
            let messages =
                convert_response_items(&items, RigProtocol::Anthropic, "source", &mut replay)
                    .expect("convert history");
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| matches!(message, Message::Assistant { .. }))
                    .count(),
                2
            );
            assert_eq!(
                replay,
                vec![crate::hosted_replay::ReplayGroup {
                    index: 1,
                    blocks: expected_blocks.clone(),
                    cited_text: Vec::new(),
                }]
            );
            assert!(matches!(messages.last(), Some(Message::Assistant { .. })));
        }
    }
}

#[test]
fn hosted_replay_after_tool_result_does_not_attach_to_the_tool_call_turn() {
    let expected_blocks = json!([
        {"type": "server_tool_use", "id": "search-2", "name": "web_search",
         "input": {"query": "after tool"}}
    ])
    .as_array()
    .expect("blocks")
    .clone();
    let blocks = envelope(json!(expected_blocks));
    let items = serde_json::from_value::<Vec<ResponseItem>>(json!([
        {"type":"function_call", "name":"lookup", "call_id":"call-1", "arguments":"{}"},
        {"type":"function_call_output", "call_id":"call-1", "output":"result"},
        {"type":"web_search_call", "wire_blocks":blocks}
    ]))
    .expect("history");
    let mut replay = Vec::new();
    let messages = convert_response_items(&items, RigProtocol::Anthropic, "source", &mut replay)
        .expect("convert history");
    assert_eq!(messages.len(), 3);
    assert_eq!(
        replay,
        vec![crate::hosted_replay::ReplayGroup {
            index: 1,
            blocks: expected_blocks.clone(),
            cited_text: Vec::new(),
        }]
    );
    let Some(Message::Assistant { content, .. }) = messages.last() else {
        panic!("search assistant");
    };
    assert_eq!(content.len(), 1);
    let AssistantContent::Text(anchor) = &content[0] else {
        panic!("SDK raw-content anchor");
    };
    assert_eq!(anchor.text, "");
    assert_eq!(
        anchor
            .additional_params
            .as_ref()
            .and_then(|params| params.get("anthropic_content")),
        expected_blocks.first()
    );
}

#[test]
fn late_result_only_projection_uses_the_validated_original_call_as_its_sdk_anchor() {
    let call = json!({"type":"server_tool_use", "id":"late", "name":"web_search", "input":{}, "vendor":"preserve"});
    let result = json!({"type":"tool_result", "tool_use_id":"late", "content":"GLM result", "vendor":"preserve"});
    let items = serde_json::from_value::<Vec<ResponseItem>>(json!([
        {"type":"web_search_call", "wire_blocks":envelope(json!([call]))},
        {"type":"message", "role":"user", "content":[{"type":"input_text", "text":"next"}]},
        {"type":"web_search_call", "wire_blocks":envelope(json!([call, result]))},
    ]))
    .expect("history");
    let mut replay = Vec::new();
    let messages = convert_response_items(&items, RigProtocol::Anthropic, "source", &mut replay)
        .expect("convert late result");
    assert_eq!(
        replay,
        vec![
            crate::hosted_replay::ReplayGroup {
                index: 0,
                blocks: vec![call.clone()],
                cited_text: Vec::new()
            },
            crate::hosted_replay::ReplayGroup {
                index: 1,
                blocks: vec![result],
                cited_text: Vec::new()
            },
        ]
    );
    let Some(Message::Assistant { content, .. }) = messages.last() else {
        panic!("late assistant")
    };
    let AssistantContent::Text(anchor) = &content[0] else {
        panic!("raw call anchor")
    };
    assert_eq!(
        anchor
            .additional_params
            .as_ref()
            .and_then(|params| params.get("anthropic_content")),
        Some(&call)
    );
}

#[test]
fn legacy_and_foreign_payloads_do_not_replay_and_create_no_assistant() {
    for (label, payload) in [
        (
            "legacy bare array",
            json!([
                {"type": "server_tool_use", "id": "old", "name": "web_search", "input": {}}
            ]),
        ),
        (
            "foreign source",
            json!({"version": 1, "source": "Anthropic:other", "blocks": [
                {"type": "server_tool_use", "id": "x", "name": "web_search", "input": {}}
            ]}),
        ),
        ("unrecognized shape", json!({"unexpected": true})),
        (
            "malformed call input",
            envelope(json!([
                {"type":"server_tool_use", "id":"x", "name":"web_search", "input":[]}
            ])),
        ),
        (
            "missing call name",
            envelope(json!([
                {"type":"server_tool_use", "id":"x", "input":{}}
            ])),
        ),
        (
            "oversized loaded call",
            envelope(json!([
                {"type":"server_tool_use", "id":"x", "name":"web_search", "input":{"query":"x".repeat(crate::hosted_replay::MAX_PAIR_BYTES)}}
            ])),
        ),
    ] {
        let items = serde_json::from_value::<Vec<ResponseItem>>(json!([
            {"type":"message", "role":"user", "content":[{"type":"input_text", "text":"q"}]},
            {"type":"web_search_call", "wire_blocks":payload}
        ]))
        .expect("history");
        let mut replay = Vec::new();
        let messages =
            convert_response_items(&items, RigProtocol::Anthropic, "source", &mut replay)
                .expect("convert history");
        assert!(replay.is_empty(), "{label} must not replay");
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::Assistant { .. })),
            "{label} must not synthesize an assistant message"
        );
    }
}
