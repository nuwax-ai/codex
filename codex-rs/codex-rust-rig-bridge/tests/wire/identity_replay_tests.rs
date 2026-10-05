//! v3 ownership and ordering through real streamed events and HTTP requests.
use super::error_tests::provider;
use super::hosted_tools_wire_tests::SearchReplay;
use super::hosted_tools_wire_tests::capture_search_replay;
use super::support;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ResponseItem;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn call(id: &str) -> Value {
    json!({"type":"server_tool_use","id":id,"name":"web_search","input":{}})
}
fn result(id: &str) -> Value {
    json!({"type":"web_search_tool_result","tool_use_id":id,"content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC"}]})
}
fn text(value: &str) -> Value {
    json!({"type":"text","text":value})
}
fn cited(value: &str) -> Value {
    json!({"type":"text","text":value,"citations":[{"type":"web_search_result_location","cited_text":"finding","url":"https://example.com","title":"Example","encrypted_index":"ENC_INDEX"}]})
}
fn message(response: &str, value: &str) -> Value {
    json!({"type":"message","id":format!("rigseg_{response}_0"),"role":"assistant","content":[{"type":"output_text","text":value}]})
}
fn carrier(
    response: &str,
    blocks: Vec<Value>,
    indices: Vec<u64>,
    layout: Option<Vec<Value>>,
) -> Value {
    let mut value = json!({"version":3,"source":"test-current","response_id":response,"blocks":blocks,"block_indices":indices});
    if let Some(layout) = layout {
        value["layout"] = json!(layout);
    }
    value
}
fn layout(response: &str, value: &str) -> Vec<Value> {
    vec![
        json!({"kind":"pair","index":0}),
        json!({"kind":"pair","index":1}),
        json!({"kind":"cited","index":2,"owner":format!("rigseg_{response}_0"),"block":cited(value)}),
    ]
}

pub(super) fn response(id: &str, blocks: Vec<Value>, stop: &str) -> String {
    let mut frames = vec![
        json!({"type":"message_start","message":{"id":id,"type":"message","role":"assistant","content":[],"model":"review-model","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":9,"output_tokens":0}}}),
    ];
    for (index, block) in blocks.into_iter().enumerate() {
        let mut start = block.clone();
        match block["type"].as_str().unwrap() {
            "text" => start["text"] = json!(""),
            "thinking" => {
                start["thinking"] = json!("");
                start["signature"] = json!("");
            }
            _ => {}
        }
        frames.push(json!({"type":"content_block_start","index":index,"content_block":start}));
        match block["type"].as_str().unwrap() {
            "text"=>frames.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":block["text"]}})),
            "thinking"=>{
                frames.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":block["thinking"]}}));
                frames.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"signature_delta","signature":block["signature"]}}));
            },
            _=>{},
        }
        frames.push(json!({"type":"content_block_stop","index":index}));
    }
    frames.push(json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},"usage":{"output_tokens":12}}));
    frames.push(json!({"type":"message_stop"}));
    frames
        .into_iter()
        .map(|frame| {
            format!(
                "event: {}\ndata: {frame}\n\n",
                frame["type"].as_str().unwrap()
            )
        })
        .collect()
}

async fn capture_and_replay(attempts: Vec<String>) -> (Vec<Value>, Vec<ResponseItem>) {
    let mut payloads = attempts;
    payloads.push(support::ANTHROPIC_SSE.into());
    let (address, server) = support::sequence_server(payloads).await;
    let provider = provider(address);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search"},{"type":"function","name":"lookup","parameters":{"type":"object","properties":{}}}]),
    );
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let mut items = Vec::new();
    while let Some(event) = stream.next().await {
        if let ResponseEvent::OutputItemDone(item) = event.unwrap() {
            items.push(item);
        }
    }
    // Actual IDs and envelopes cross the same serde boundary as a rollout.
    let saved = serde_json::to_vec(&items).unwrap();
    let loaded: Vec<ResponseItem> = serde_json::from_slice(&saved).unwrap();
    request.input.extend(loaded);
    request
        .input
        .push(serde_json::from_value(support::user()).unwrap());
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    (server.await.unwrap(), items)
}

fn assistant(body: &Value) -> Vec<Value> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .flat_map(|message| message["content"].as_array().unwrap())
        .cloned()
        .collect()
}

#[tokio::test]
async fn text_only_and_pair_only_pauses_preserve_true_response_ownership_after_reload() {
    for text_only in [true, false] {
        let early = if text_only {
            vec![text("same")]
        } else {
            vec![call("s")]
        };
        let late = if text_only {
            vec![call("s"), result("s"), cited("same")]
        } else {
            vec![result("s"), cited("same")]
        };
        let (bodies, items) = capture_and_replay(vec![
            response("early", early, "pause_turn"),
            response("late", late, "end_turn"),
        ])
        .await;
        assert_eq!(bodies.len(), 3);
        let mut expected = if text_only {
            vec![text("same")]
        } else {
            Vec::new()
        };
        expected.extend([call("s"), result("s"), cited("same")]);
        assert_eq!(assistant(&bodies[2]), expected, "text_only={text_only}");
        let owners: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                ResponseItem::Message { id: Some(id), .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert!(owners.iter().all(|id| id.starts_with("rigseg_")));
        assert!(
            owners
                .windows(2)
                .all(|pair| pair[0].split('_').nth(1) != pair[1].split('_').nth(1))
        );
    }
}

#[tokio::test]
async fn full_carrier_drop_keeps_all_sibling_pairs_and_plain_history_once() {
    let long = "x".repeat(9_400);
    let (bodies, items) = capture_and_replay(vec![response(
        "large",
        vec![text(&long), call("a"), result("a"), call("b"), result("b")],
        "end_turn",
    )])
    .await;
    let payloads: Vec<_> = items
        .iter()
        .filter_map(|item| match item {
            ResponseItem::WebSearchCall {
                wire_blocks: Some(payload),
                ..
            } => Some(payload),
            _ => None,
        })
        .collect();
    assert_eq!(payloads.len(), 2);
    assert!(
        payloads
            .iter()
            .all(|payload| payload.get("layout").is_none())
    );
    assert_eq!(payloads[0]["response_id"], payloads[1]["response_id"]);
    assert_eq!(
        assistant(bodies.last().unwrap()),
        vec![text(&long), call("a"), result("a"), call("b"), result("b")]
    );
}

#[tokio::test]
async fn reasoning_signature_and_client_tool_boundaries_replay_in_wire_order() {
    let thought = json!({"type":"thinking","thinking":"thought","signature":"signed"});
    let tool = json!({"type":"tool_use","id":"client","name":"lookup","input":{}});
    let blocks = vec![
        text("intro"),
        call("s"),
        thought,
        tool,
        result("s"),
        cited("answer"),
    ];
    let (bodies, _) = capture_and_replay(vec![response("mixed", blocks.clone(), "end_turn")]).await;
    assert_eq!(assistant(bodies.last().unwrap()), blocks);
}

#[tokio::test]
async fn corrupt_layout_coverage_keeps_each_pair_once_without_citation_attribution() {
    for (indices, rejected) in [
        (vec![0, 0, 1], true),
        (vec![0], false),
        (vec![0, 1, 99], false),
    ] {
        let bad_layout: Vec<_> = indices
            .into_iter()
            .map(|index| json!({"kind":"pair","index":index}))
            .collect();
        let payload = carrier(
            "r",
            vec![call("s"), result("s")],
            vec![0, 1],
            Some(bad_layout),
        );
        let messages = capture_search_replay(
            vec![
                support::user(),
                message("r", "same"),
                json!({"type":"web_search_call","wire_blocks":payload}),
                support::user(),
            ],
            SearchReplay::Enabled,
        )
        .await;
        let content = messages
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "assistant")
            .unwrap()["content"]
            .clone();
        let expected = if rejected {
            vec![text("same")]
        } else {
            vec![text("same"), call("s"), result("s")]
        };
        assert_eq!(content, json!(expected));
    }
}

#[tokio::test]
async fn late_foreign_calls_survive_replay_when_the_original_envelope_is_missing() {
    for original_survives in [false, true] {
        for layout_survives in [false, true] {
            let original = original_survives.then(|| {
                carrier(
                    "early",
                    vec![call("foreign")],
                    vec![0],
                    /*layout*/ None,
                )
            });
            let late_layout = layout_survives.then(|| {
                vec![
                    json!({"kind":"text","index":0,"owner":"rigseg_late_0","block":text("intro")}),
                    json!({"kind":"pair","index":1}),
                ]
            });
            let late = carrier(
                "late",
                vec![call("foreign"), result("foreign")],
                vec![u64::MAX, 1],
                late_layout,
            );
            // Keep the earlier visible history even when its pending call's
            // replay envelope was lost. The late response carries a clone,
            // which must be accepted once only if the original is unavailable.
            let messages = capture_search_replay(
                vec![
                    support::user(),
                    message("early", "before"),
                    json!({"type":"web_search_call","status":"in_progress","wire_blocks":original}),
                    support::user(),
                    message("late", "intro"),
                    json!({"type":"web_search_call","status":"completed","wire_blocks":late}),
                    support::user(),
                ],
                SearchReplay::Enabled,
            )
            .await;
            let mut early_content = vec![text("before")];
            if original_survives {
                early_content.push(call("foreign"));
            }
            let late_content = match (original_survives, layout_survives) {
                (false, true) => vec![call("foreign"), text("intro"), result("foreign")],
                (false, false) => vec![text("intro"), call("foreign"), result("foreign")],
                (true, false) | (true, true) => vec![text("intro"), result("foreign")],
            };
            let user = json!({"role":"user","content":[text("hello")]});
            assert_eq!(
                messages,
                json!([
                    user.clone(),
                    {"role":"assistant","content":early_content},
                    user.clone(),
                    {"role":"assistant","content":late_content},
                    user,
                ]),
                "original_survives={original_survives}, layout_survives={layout_survives}"
            );
        }
    }
}

#[tokio::test]
async fn deduped_layout_carriers_cannot_inject_unbounded_text_or_citations() {
    let mut items = vec![support::user()];
    for n in 0..100 {
        let response = format!("r{n}");
        items.push(message(&response, "same"));
        items.push(
            json!({"type":"web_search_call","wire_blocks":carrier(&response,
            vec![call("s"),result("s")],vec![0,1],Some(layout(&response,"same")))}),
        );
    }
    items.push(support::user());
    let messages = capture_search_replay(items, SearchReplay::Enabled).await;
    let content = messages
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap()["content"]
        .as_array()
        .unwrap();
    let mut expected = vec![call("s"), result("s"), cited("same")];
    expected.extend((1..100).map(|_| text("same")));
    assert_eq!(content, &expected);
}
