use super::*;
use codex_protocol::protocol::TokenUsage;
use serde_json::json;

#[tokio::test]
async fn failed_tool_scenario_preserves_the_complete_first_turn() {
    let cfg = LiveConfig {
        vendor: "scripted".into(),
        api_key: "unused".into(),
        base_url: "http://unused.invalid".into(),
        anthropic_base_url: None,
        responses_base_url: None,
        model: "test-model".into(),
    };
    let completed: Vec<ResponseItem> = serde_json::from_value(json!([
        {"type":"reasoning","summary":[],"encrypted_content":"signed-envelope"},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"checking weather"}]},
        {"type":"function_call","id":"item-weather","call_id":"call-weather","name":"get_weather","arguments":"{\"city\":\"北京\"}"}
    ]))
    .expect("first-turn items");
    let terminal = ResponseEvent::Completed {
        response_id: "scripted-turn".into(),
        token_usage: Some(TokenUsage {
            input_tokens: 1,
            output_tokens: 1,
            total_tokens: 2,
            ..Default::default()
        }),
        usage_metadata: None,
        end_turn: Some(true),
    };
    let mut first_turn = vec![
        ResponseEvent::Created { response_id: None },
        ResponseEvent::OutputItemAdded(completed[1].clone()),
        ResponseEvent::OutputTextDelta("checking weather".into()),
    ];
    first_turn.extend(completed.iter().cloned().map(ResponseEvent::OutputItemDone));
    first_turn.push(terminal.clone());
    let mut replies = [
        first_turn,
        vec![
            ResponseEvent::OutputTextDelta("tool failed".into()),
            terminal,
        ],
    ]
    .into_iter();
    let mut requests = Vec::new();
    scenario_anthropic_tool_error_with_runner(&cfg, Bridge::Rig, |request, tag| {
        requests.push((tag, request));
        std::future::ready(replies.next().expect("exactly two turns"))
    })
    .await;

    assert_eq!(
        requests.iter().map(|(tag, _)| *tag).collect::<Vec<_>>(),
        vec!["anthropic-tool-err-t1", "anthropic-tool-err-t2"]
    );
    let mut expected = requests[0].1.input.clone();
    expected.extend(completed);
    expected.push(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some("call-weather".into()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                r#"{"error":"weather service unavailable (simulated)"}"#.into(),
            ),
            success: Some(false),
        },
        internal_chat_message_metadata_passthrough: None,
    });
    assert_eq!(requests[1].1.input, expected);
}
