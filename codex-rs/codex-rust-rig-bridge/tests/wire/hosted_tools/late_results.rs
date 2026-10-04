use super::common::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn anthropic_late_search_results_preserve_response_positions_and_raw_fields() {
    for result_type in ["web_search_tool_result", "tool_result"] {
        for plain_message in [false, true] {
            for identity in [false, true] {
                let mut pair = search_pair("late");
                pair[1]["type"] = json!(result_type);
                if result_type == "tool_result" {
                    pair[1]["content"] = json!(
                        "[{'text': [{'title': 'GLM result', 'link': 'https://example.com'}]}]"
                    );
                }
                pair[1]["vendor_result"] =
                    json!({"opaque":"terminal", "number":9007199254740993u64});
                let cited = json!({"type":"text", "text":"late answer", "citations":[], "vendor_cite":"preserve"});
                let mut items = vec![
                    support::user(),
                    json!({"type":"function_call", "name":"lookup", "call_id":"client", "arguments":"{}"}),
                    json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
                    json!({"type":"function_call_output", "call_id":"client", "output":"client result"}),
                ];
                if plain_message {
                    items.push(json!({"type":"message", "id":"rigseg_late_0", "role":"assistant", "content":[{"type":"output_text", "text":"late answer"}]}));
                }
                // v2 payloads carry block identity: the result streamed at
                // wire index 0 of the late response (the cloned call is
                // foreign), and the cited block at index 1, raw fields and
                // all. v1 payloads carry only the pair plus unattributable
                // cited text.
                let winner = if identity {
                    json!({
                        "version":3, "source":"test-current", "response_id":"late",
                        "blocks":pair,
                        "block_indices":[u64::MAX, 0],
                        "layout":[
                            {"kind":"pair","index":0},
                            {"kind":"cited","index":1,"owner":"rigseg_late_0","block":cited},
                        ],
                    })
                } else {
                    json!({"version":1, "source":"test-current", "blocks":pair, "cited_text":[cited.clone()]})
                };
                items.extend([
                    json!({"type":"web_search_call", "wire_blocks":winner}),
                    support::user(),
                ]);
                let late_content = if identity && plain_message {
                    // Identity rebuild: the result at its response position
                    // and the raw cited block replacing (or supplying) the
                    // merged text — vendor fields verbatim either way.
                    vec![pair[1].clone(), cited]
                } else if plain_message {
                    // No identity: the plain text stays untouched and the
                    // unattributable cited text is not injected.
                    vec![json!({"type":"text","text":"late answer"}), pair[1].clone()]
                } else {
                    vec![pair[1].clone()]
                };
                let user = json!({"role":"user", "content":[{"type":"text", "text":"hello"}]});
                assert_eq!(
                    capture_search_replay(items, SearchReplay::Enabled).await,
                    json!([
                        user.clone(),
                        {"role":"assistant", "content":[{"type":"tool_use", "id":"client", "name":"lookup", "input":{}}, pair[0].clone()]},
                        {"role":"user", "content":[{"type":"tool_result", "tool_use_id":"client", "content":[{"type":"text", "text":"client result"}]}]},
                        {"role":"assistant", "content":late_content},
                        user,
                    ]),
                    "{result_type}, plain_message={plain_message}, identity={identity}"
                );
            }
        }
    }
}

/// R3: a mixed server/client turn — the server tool call arrives in
/// response 1 (pending), the client tool returns, and ONLY the search
/// result arrives in response 2. The result must close the pending call
/// from history as a NEW completed item, and the following request must
/// replay the pair exactly once (no duplicate call block).
#[tokio::test]
async fn mixed_turn_result_arriving_in_the_next_response_closes_the_pending_call() {
    let sse_with_pending_call = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_mixed\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"mixed\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"lookup\",\"input\":{}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":6}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let sse_with_late_result = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_mixed\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_LATE\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) = support::sequence_server(vec![
        sse_with_pending_call,
        sse_with_late_result.clone(),
        support::ANTHROPIC_SSE.to_string(),
    ])
    .await;
    let provider = provider(address);
    let source = codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "review-model",
        Some("legacy-unscoped"),
    )
    .expect("source identity");
    let pending_call = json!([
        {"type":"server_tool_use","id":"srvu_mixed","name":"web_search","input":{"query":"mixed"}}
    ]);
    let mut search_events: Vec<serde_json::Value> = Vec::new();

    // Request 1: the pending call is emitted in_progress.
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search"}, {"type":"function","name":"lookup"}]),
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
    while let Some(event) = stream.next().await {
        if let Ok(codex_api::ResponseEvent::OutputItemDone(item)) = event
            && let ResponseItem::WebSearchCall { status, .. } = &item
        {
            search_events.push(json!({"phase": 1, "status": status}));
        }
    }

    // Request 2: the history carries the client tool result plus the pending
    // search item; the response delivers ONLY the late result.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        serde_json::from_value(json!({
            "type":"function_call","name":"lookup","call_id":"tool_1","arguments":"{}"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"function_call_output","call_id":"tool_1","output":"client result"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_mixed","status":"in_progress",
            "action":{"type":"search","query":"mixed"},
            "wire_blocks":{"version":1,"source":source,"blocks":pending_call}
        }))
        .unwrap(),
    ];
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
    let mut completed_pair = None;
    while let Some(event) = stream.next().await {
        if let Ok(codex_api::ResponseEvent::OutputItemDone(item)) = event
            && let ResponseItem::WebSearchCall {
                wire_blocks,
                status,
                ..
            } = item
        {
            search_events.push(json!({"phase": 2, "status": status}));
            if status.as_deref() == Some("completed") {
                completed_pair = wire_blocks;
            }
        }
    }
    let completed_pair = completed_pair.expect("the late result closes the pending call");
    assert_eq!(
        completed_pair["blocks"],
        json!([
            {"type":"server_tool_use","id":"srvu_mixed","name":"web_search","input":{"query":"mixed"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_mixed","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_LATE"}]},
        ]),
        "the appended item carries call+result"
    );
    assert_eq!(completed_pair["source"], json!(source));

    // Request 3: history carries BOTH items (in_progress + appended
    // completed); the replay must contain the call exactly once.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_mixed","status":"in_progress",
            "action":{"type":"search","query":"mixed"},
            "wire_blocks":{"version":1,"source":source,"blocks":pending_call}
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_mixed","status":"completed",
            "action":{"type":"search","query":"mixed"},
            "wire_blocks":completed_pair
        }))
        .unwrap(),
    ];
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
    let bodies = server.await.unwrap();
    let encoded = bodies[2]["messages"].to_string();
    let call_occurrences = encoded.matches("srvu_mixed").count();
    assert_eq!(
        call_occurrences, 2,
        "call id once in the use block + once in tool_use_id: {encoded}"
    );
    assert_eq!(encoded.matches("ENC_LATE").count(), 1, "{encoded}");
}

/// R3: cited text blocks persist inside the capture envelope and replay
/// after their pair inside the same assistant group.
#[tokio::test]
async fn cited_text_blocks_persist_and_replay_after_their_pair() {
    let cited_sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_cited\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"cited\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_cited\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_CIT\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\",\"citations\":[{\"type\":\"search_result_location\",\"cited_text\":\"finding\",\"source\":\"https://example.com\",\"title\":\"Example\",\"search_result_index\":0,\"start_block_index\":1,\"end_block_index\":2}]}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"answer citing \"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"the result\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) =
        support::sequence_server(vec![cited_sse, support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
    let source = codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "review-model",
        Some("legacy-unscoped"),
    )
    .expect("source identity");
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);

    // Turn 1: the emitted search item carries the cited text in its envelope.
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
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
    let mut captured_envelope = None;
    let mut captured_message = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item @ ResponseItem::Message { .. }) => {
                captured_message = Some(item)
            }
            ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall { wire_blocks, .. }) => {
                captured_envelope = wire_blocks
            }
            _ => {}
        }
    }
    let envelope = captured_envelope.expect("search item emitted");
    let captured_message = captured_message.expect("saved message");
    let owner = match &captured_message {
        ResponseItem::Message { id: Some(id), .. } => id.as_str(),
        _ => panic!("message id"),
    };
    assert_eq!(envelope["source"], json!(source));
    assert_eq!(
        envelope["block_indices"],
        json!([0, 1]),
        "the pair blocks carry their wire indices"
    );
    assert_eq!(
        envelope["layout"],
        json!([
            {"kind":"pair","index":0},
            {"kind":"pair","index":1},
            {"kind":"cited","index":2,"owner":owner,"block":{
                "type":"text",
                "text":"answer citing the result",
                "citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]
            }},
        ]),
        "the cited text block persists verbatim in the identity layout"
    );

    // Turn 2: the cited block replays after the pair, inside the assistant.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        captured_message.clone(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_cited","status":"completed",
            "action":{"type":"search","query":"cited"},
            "wire_blocks":envelope
        }))
        .unwrap(),
    ];
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
    let bodies = server.await.unwrap();
    let messages = bodies[1]["messages"].as_array().expect("messages");
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant with replay");
    assert_eq!(
        assistant["content"],
        json!([
            {"type":"server_tool_use","id":"srvu_cited","name":"web_search","input":{"query":"cited"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_cited","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_CIT"}]},
            {"type":"text","text":"answer citing the result","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]},
        ]),
        "pair first, cited text after, verbatim"
    );
}

/// N2: the assistant's plain answer and the envelope's cited text describe
/// the SAME streamed text; the projection must carry it exactly once, with
/// the citations attached at the answer's position.
#[tokio::test]
async fn cited_text_replaces_the_plain_answer_projection_in_place() {
    let cited_sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_once\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"once\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_once\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_ONCE\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\",\"citations\":[{\"type\":\"search_result_location\",\"cited_text\":\"finding\",\"source\":\"https://example.com\",\"title\":\"Example\",\"search_result_index\":0,\"start_block_index\":1,\"end_block_index\":2}]}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"the single answer\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":5}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) =
        support::sequence_server(vec![cited_sse, support::ANTHROPIC_SSE.to_string()]).await;
    let provider = provider(address);
    let _source = codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "review-model",
        Some("legacy-unscoped"),
    )
    .expect("source identity");
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);

    // Turn 1 captures the envelope (pair + cited text).
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
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
    let mut captured_envelope = None;
    let mut captured_message = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item @ ResponseItem::Message { .. }) => {
                captured_message = Some(item)
            }
            ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall { wire_blocks, .. }) => {
                captured_envelope = wire_blocks
            }
            _ => {}
        }
    }
    let envelope = captured_envelope.expect("search item emitted");
    let captured_message = captured_message.expect("saved message");

    // Turn 2 replays the REAL history shape: the saved assistant Message
    // with the plain answer AND the search item with its envelope.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        captured_message,
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_once","status":"completed",
            "action":{"type":"search","query":"once"},
            "wire_blocks":envelope
        }))
        .unwrap(),
    ];
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
    let bodies = server.await.unwrap();
    let messages = bodies[1]["messages"].as_array().expect("messages");
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("assistant with replay");
    assert_eq!(
        assistant["content"],
        json!([
            {"type":"server_tool_use","id":"srvu_once","name":"web_search","input":{"query":"once"}},
            {"type":"web_search_tool_result","tool_use_id":"srvu_once","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_ONCE"}]},
            {"type":"text","text":"the single answer","citations":[{"type":"search_result_location","cited_text":"finding","source":"https://example.com","title":"Example","search_result_index":0,"start_block_index":1,"end_block_index":2}]},
        ]),
        "the cited terminal state replaces the plain answer at its position"
    );
    assert_eq!(
        assistant["content"]
            .to_string()
            .matches("the single answer")
            .count(),
        1,
        "the answer must appear exactly once"
    );
}

/// N3: in the production history order — client call, pending server call,
/// client output, late completed item — the request prefix sent while the
/// search was pending stays byte-identical after the result arrives; the
/// late result joins only its own new response position.
#[tokio::test]
async fn mixed_turn_late_result_preserves_the_sent_request_prefix() {
    let sse_pending = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_prefix\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"prefix\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_p\",\"name\":\"lookup\",\"input\":{}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":6}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let sse_late_result = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m2\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"m\",\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":4,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_prefix\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC_PREFIX\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":2}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string();
    let (address, server) = support::sequence_server(vec![
        sse_pending,
        sse_late_result,
        support::ANTHROPIC_SSE.to_string(),
    ])
    .await;
    let provider = provider(address);
    let source = codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "review-model",
        Some("legacy-unscoped"),
    )
    .expect("source identity");
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let pending_call = json!([
        {"type":"server_tool_use","id":"srvu_prefix","name":"web_search","input":{"query":"prefix"}}
    ]);

    // Request 1 emits the pending call in_progress (nothing asserted here
    // beyond draining the stream).
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search"}, {"type":"function","name":"lookup"}]),
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }

    // Request 2 carries the REAL production order: client call, client
    // output, then the pending server call. This is the prefix the model
    // saw while the search was pending.
    request.input = vec![
        serde_json::from_value(support::user()).unwrap(),
        serde_json::from_value(json!({
            "type":"function_call","name":"lookup","call_id":"tool_p","arguments":"{}"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"function_call_output","call_id":"tool_p","output":"client result"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_prefix","status":"in_progress",
            "action":{"type":"search","query":"prefix"},
            "wire_blocks":{"version":1,"source":source,"blocks":pending_call}
        }))
        .unwrap(),
    ];
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
    let mut completed_pair = None;
    while let Some(event) = stream.next().await {
        if let Ok(codex_api::ResponseEvent::OutputItemDone(item)) = event
            && let ResponseItem::WebSearchCall {
                wire_blocks,
                status,
                ..
            } = item
            && status.as_deref() == Some("completed")
        {
            completed_pair = wire_blocks;
        }
    }
    let completed_pair = completed_pair.expect("the late result closes the pending call");

    // Request 3: same history plus the appended completed item.
    request.input.push(
        serde_json::from_value(json!({
            "type":"web_search_call","id":"srvu_prefix","status":"completed",
            "action":{"type":"search","query":"prefix"},
            "wire_blocks":completed_pair
        }))
        .unwrap(),
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let bodies = server.await.unwrap();
    let prefix = bodies[1]["messages"]
        .as_array()
        .expect("request 2 messages");
    let after = bodies[2]["messages"]
        .as_array()
        .expect("request 3 messages");
    // The late result joins the pending call's own response position: every
    // message BEFORE that assistant stays byte-identical, and the assistant
    // keeps its already-sent content as a prefix (use first, result after).
    assert_eq!(
        after.len(),
        prefix.len(),
        "the late result extends its assistant, not the message list"
    );
    assert_eq!(
        after[..prefix.len() - 1],
        prefix[..prefix.len() - 1],
        "messages before the pending call's assistant stay byte-identical: {} vs {}",
        serde_json::to_string(&after).unwrap(),
        serde_json::to_string(prefix).unwrap(),
    );
    let pending_assistant = prefix
        .last()
        .expect("the pending call's assistant was already sent");
    let extended_assistant = after
        .last()
        .expect("the same assistant carries the late result");
    assert_eq!(
        pending_assistant["role"],
        json!("assistant"),
        "the pending call projected into an assistant message"
    );
    let pending_content = pending_assistant["content"]
        .as_array()
        .expect("pending assistant content");
    let extended_content = extended_assistant["content"]
        .as_array()
        .expect("extended assistant content");
    assert!(
        extended_content.starts_with(pending_content),
        "the already-sent assistant content stays a stable prefix: {} then {}",
        serde_json::to_string(pending_content).unwrap(),
        serde_json::to_string(extended_content).unwrap(),
    );
    // The client call/output never left the projection.
    let encoded = serde_json::to_string(&after).unwrap();
    assert!(encoded.contains("\"tool_p\""), "{encoded}");
    assert!(encoded.contains("client result"), "{encoded}");
    // The use block stays in the pending assistant; the result joins that
    // same response position after it.
    let completed_assistant = after
        .last()
        .expect("the pending call's assistant carries the late result");
    assert_eq!(completed_assistant["role"], json!("assistant"));
    let completed_content = completed_assistant["content"].to_string();
    assert!(
        completed_content.contains("ENC_PREFIX"),
        "{completed_content}"
    );
    let use_index = completed_content.find("server_tool_use").expect("use kept");
    let result_index = completed_content
        .find("ENC_PREFIX")
        .expect("result present");
    assert!(
        use_index < result_index,
        "the use stays at its position ahead of the late result"
    );
}
