//! Resume actual Chat bridge outputs through the Responses request path.

use codex_api::ResponseEvent;
use codex_live_tests::Bridge;
use codex_live_tests::LiveConfig;
use codex_live_tests::LiveWire;
use codex_live_tests::responses_url_or_skip;
use codex_live_tests::run_turn;
use codex_live_tests::user_message;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;

pub(super) async fn scenario_responses_cross_protocol(cfg: &LiveConfig) {
    let Some(responses_url) = responses_url_or_skip(cfg) else {
        return;
    };
    let ctx = format!("{}/rig responses-xproto", cfg.vendor);
    let source = codex_live_tests::load_rig_event_fixture(&cfg.vendor, "tool-t1")
        .expect("recorded Chat tool turn for cross-protocol history");
    let source_events = codex_rust_rig_bridge::replay_fixture_events(&source)
        .expect("convert actual recorded Rig Chat events into history");
    let source_turn = codex_live_tests::load_fixture(&cfg.vendor, "rig", "tool-t1")
        .expect("recorded Chat request for cross-protocol history");
    let mut request = super::base_request(
        cfg,
        "Read the tool result in the conversation and reply with its report_token exactly. Do not add any other words.",
        "",
    );
    request.input = serde_json::from_value(source_turn.request["input"].clone())
        .expect("recorded Chat input items");
    super::append_turn_outputs(&mut request.input, &source_events);
    assert!(
        request.input.iter().any(|item| matches!(
            item,
            ResponseItem::Reasoning {
                summary,
                content: Some(content),
                encrypted_content: Some(envelope),
                ..
            } if summary.is_empty()
                && codex_rust_rig_bridge::is_replay_envelope(envelope)
                && content.iter().any(|part| matches!(
                    part,
                    ReasoningItemContent::ReasoningText { text } if !text.is_empty()
                ))
        )),
        "{ctx}: source must contain real Rig reasoning_text and its replay envelope"
    );
    let calls: Vec<_> = source_events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, call_id, .. }) => {
                Some((name.as_str(), call_id.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "{ctx}: source fixture must have one tool call"
    );
    let (name, call_id) = &calls[0];
    assert_eq!(*name, "get_weather");

    // Only the tool result contains this token. It is stable for cassette
    // replay, and cannot be recovered from the user prompt or source events.
    let marker = format!("XPROTO_{}_7d31a94e", cfg.vendor);
    assert!(
        !serde_json::to_string(&request)
            .expect("serialize history before tool result")
            .contains(&marker),
        "{ctx}: marker must appear only in the tool result"
    );
    request.input.push(ResponseItem::FunctionCallOutput {
        id: None,
        call_id: Some(call_id.clone()),
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(
                serde_json::json!({
                    "city": "北京",
                    "condition": "晴",
                    "temp_c": 22,
                    "report_token": marker,
                })
                .to_string(),
            ),
            success: Some(true),
        },
        internal_chat_message_metadata_passthrough: None,
    });
    request.input.push(user_message(
        "请逐字复制工具结果中的 report_token 字段，只输出该字段的值。",
    ));
    let events = run_turn(
        cfg,
        &responses_url,
        LiveWire::Responses,
        Bridge::Rig,
        &request,
        "responses-xproto",
    )
    .await;
    let output: String = events
        .iter()
        .filter_map(|event| match event {
            ResponseEvent::OutputTextDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        output.trim(),
        marker,
        "{ctx}: the Responses answer must read the paired Chat tool result"
    );
    codex_live_tests::assert_completed_with_usage(&events, &ctx);
}
