use super::common::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn anthropic_server_tool_use_maps_to_a_web_search_call_item() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let payload = glm_web_search_turn();
    let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
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
    let mut done_items = Vec::new();
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item) => done_items.push(item),
            ResponseEvent::Completed { .. } => completed += 1,
            _ => {}
        }
    }
    server.await.unwrap();
    assert_eq!(completed, 1, "turn must complete: {done_items:?}");
    // Exactly one web search call (completed, GLM query field) and one text
    // message; the non-standard assistant-side tool_result block must not
    // surface as an item or an empty message.
    let web_search_calls: Vec<&ResponseItem> = done_items
        .iter()
        .filter(|item| matches!(item, ResponseItem::WebSearchCall { .. }))
        .collect();
    assert_eq!(web_search_calls.len(), 1, "items: {done_items:?}");
    let source = codex_rust_rig_bridge::reasoning_source_with_auth_domain(
        &provider,
        RigProtocol::Anthropic,
        "review-model",
        Some("legacy-unscoped"),
    )
    .expect("source identity");
    let message_id = done_items
        .iter()
        .find_map(|item| match item {
            ResponseItem::Message { id: Some(id), .. } => Some(id.as_str()),
            _ => None,
        })
        .expect("persisted text owner");
    let response_id = message_id
        .strip_prefix("rigseg_")
        .unwrap()
        .rsplit_once('_')
        .unwrap()
        .0;
    assert_eq!(
        serde_json::to_value(web_search_calls[0]).unwrap(),
        json!({
            "type":"web_search_call",
            "id":"srvu_glm",
            "status":"completed",
            "action":{"type":"search","query":"上海天气"},
            // D1+R2: the raw wire pair rides the item inside the versioned
            // same-source envelope — GLM's non-standard assistant-side
            // tool_result included, verbatim — plus the v2 block identity
            // (indices + layout) replay rebuilds positions from.
            "wire_blocks":{
                "version":3,
                "response_id":response_id,
                "source":source,
                "block_indices":[0,1],
                "blocks":[
                    {"type":"server_tool_use","id":"srvu_glm","name":"web_search_prime","input":{"search_query":"上海天气","location":"cn"}},
                    {"type":"tool_result","tool_use_id":"srvu_glm","content":"[{'text': [{'title': 'weather', 'link': 'https://example.com'}]}]"},
                ],
                "layout":[
                    {"kind":"pair","index":0},
                    {"kind":"pair","index":1},
                    {"kind":"text","index":2,"owner":message_id,"block":{"type":"text","text":"上海今天多云。"}},
                ],
            },
        })
    );
    let messages: Vec<&ResponseItem> = done_items
        .iter()
        .filter(|item| matches!(item, ResponseItem::Message { .. }))
        .collect();
    assert_eq!(messages.len(), 1, "items: {done_items:?}");
    assert_eq!(
        serde_json::to_value(messages[0]).unwrap()["content"],
        json!([{"type":"output_text","text":"上海今天多云。"}])
    );
}

#[tokio::test]
async fn anthropic_server_tool_without_a_codex_item_is_ignored() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let payload = frames_sse(vec![
        json!({"type":"message_start","message":{"id":"msg-bash","type":"message","role":"assistant","content":[],"model":"m","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":2,"output_tokens":0}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use","id":"srvu_bash","name":"bash_20250124","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"ls\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"done"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ]);
    let server = tokio::spawn(async move { support::serve_payload(&listener, &payload).await });
    let request = support::request(vec![support::user()]);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
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
    let mut done_items = Vec::new();
    let mut completed = 0;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            ResponseEvent::OutputItemDone(item) => done_items.push(item),
            ResponseEvent::Completed { .. } => completed += 1,
            _ => {}
        }
    }
    server.await.unwrap();
    assert_eq!(completed, 1);
    assert!(
        done_items
            .iter()
            .all(|item| !matches!(item, ResponseItem::WebSearchCall { .. })),
        "unmapped server tools must not become web search calls: {done_items:?}"
    );
    assert_eq!(done_items.len(), 1, "only the text message: {done_items:?}");
}
