use super::pending_calls_from_wire;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn pending_search_map_tracks_exact_sent_call_and_completion_order() {
    let old = json!({"type":"server_tool_use","id":"same-id","name":"web_search","input":{"query":"old"}});
    let result = json!({"type":"web_search_tool_result","tool_use_id":"same-id","content":[]});
    let current = json!({"type":"server_tool_use","id":"same-id","name":"web_search","input":{"query":"current"}});
    let blocks = vec![
        old,
        result.clone(),
        current.clone(),
        json!({"type":"server_tool_use","id":"code","name":"bash_code_execution","input":{}}),
    ];
    assert_eq!(
        pending_calls_from_wire(blocks.iter()),
        [("same-id".into(), current)].into_iter().collect()
    );
    let mut completed = blocks;
    completed.push(result);
    assert!(pending_calls_from_wire(completed.iter()).is_empty());
}
