use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn hosted_replay_tracks_each_assistant_across_user_turns() {
    let assistant_items = [
        json!({"type":"function_call", "name":"lookup", "call_id":"call-1", "arguments":"{}"}),
        json!({"type":"custom_tool_call", "name":"custom", "call_id":"call-2", "input":"raw"}),
        json!({"type":"agent_message", "author":"worker", "recipient":"main", "content":[{"type":"input_text", "text":"result"}]}),
        json!({"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"answer"}]}),
    ];
    let blocks = json!([{"type":"server_tool_use", "id":"search-2", "name":"web_search", "input":{"query":"new turn"}}]);
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
                vec![(1, blocks.as_array().expect("blocks").clone())]
            );
            assert!(matches!(messages.last(), Some(Message::Assistant { .. })));
        }
    }
}

#[test]
fn hosted_replay_after_tool_result_does_not_attach_to_the_tool_call_turn() {
    let blocks = json!([{"type":"server_tool_use", "id":"search-2", "name":"web_search", "input":{"query":"after tool"}}]);
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
        vec![(1, blocks.as_array().expect("blocks").clone())]
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
        blocks.as_array().and_then(|blocks| blocks.first())
    );
}
