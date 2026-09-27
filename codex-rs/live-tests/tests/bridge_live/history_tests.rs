use super::*;
use serde_json::json;

#[test]
fn tool_continuation_retains_every_completed_item_in_order() {
    let original = user_message("lookup weather");
    let completed: Vec<ResponseItem> = serde_json::from_value(json!([
        {"type":"reasoning","summary":[],"encrypted_content":"signed-envelope"},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"checking"}]},
        {"type":"function_call","call_id":"one","name":"get_weather","arguments":"{}"},
        {"type":"function_call","call_id":"two","name":"get_weather","arguments":"{}"}
    ]))
    .expect("completed history fixture");
    let mut events = vec![
        ResponseEvent::Created { response_id: None },
        ResponseEvent::OutputItemAdded(completed[1].clone()),
        ResponseEvent::OutputTextDelta("checking".into()),
    ];
    events.extend(completed.iter().cloned().map(ResponseEvent::OutputItemDone));
    let mut input = vec![original.clone()];
    append_turn_outputs(&mut input, &events);
    let expected: Vec<_> = std::iter::once(original).chain(completed).collect();
    assert_eq!(input, expected);
}
