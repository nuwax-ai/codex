use super::common::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn hosted_web_search_is_injected_as_an_anthropic_server_tool() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([
            {"type":"web_search"},
            {"type":"function","name":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}}}},
        ]),
    );
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    // The hosted tool is re-declared as the Anthropic server tool next to the
    // translated function tool; neither a schema-less function entry nor the
    // raw Responses shape may leak onto the wire.
    assert_eq!(
        wire["body"]["tools"],
        json!([
            {"name":"lookup","description":"","input_schema":{"type":"object","properties":{"query":{"type":"string"}}}},
            {"type":"web_search_20250305","name":"web_search"},
        ])
    );
}

// F01: user-selected search constraints (domain allowlist, approximate
// location) must survive the Responses→Messages translation.
#[tokio::test]
async fn hosted_web_search_constraints_reach_the_anthropic_wire() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([
            {"type":"web_search","external_web_access":true,
             "filters":{"allowed_domains":["docs.rs","example.com/blog"]},
             "user_location":{"type":"approximate","city":"Shanghai","country":"CN","timezone":"Asia/Shanghai"},
             "search_context_size":"medium"},
            {"type":"function","name":"lookup","parameters":{"type":"object","properties":{"query":{"type":"string"}}}},
        ]),
    );
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    let tools = wire["body"]["tools"].as_array().unwrap().clone();
    let server_entry = tools
        .iter()
        .find(|tool| tool["type"] == "web_search_20250305")
        .expect("translated server tool on the wire");
    assert_eq!(
        server_entry["allowed_domains"],
        json!(["docs.rs", "example.com/blog"])
    );
    assert_eq!(
        server_entry["user_location"],
        json!({"type":"approximate","city":"Shanghai","country":"CN","timezone":"Asia/Shanghai"})
    );
    assert!(
        server_entry.get("search_context_size").is_none(),
        "tuning knobs without a Messages equivalent must not leak: {server_entry}"
    );
}

// F06: hosted-only requests carry no function tools, so rig's streaming path
// drops tool_choice — the transport must restore the requested choice (and
// the parallel-tool-use gate that piggybacks on it) alongside the injected
// server tools.
#[tokio::test]
async fn hosted_only_request_restores_tool_choice_and_parallel_control() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    request.parallel_tool_calls = false;
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    assert_eq!(
        wire["body"]["tool_choice"],
        json!({"type":"auto","disable_parallel_tool_use":true})
    );
    assert_eq!(
        wire["body"]["tools"],
        json!([{"type":"web_search_20250305","name":"web_search"}])
    );
}

#[tokio::test]
async fn tool_choice_none_is_restored_on_the_wire() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let mut request = support::request(vec![support::user()]);
    request.tool_choice = "none".into();
    request.parallel_tool_calls = false;
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
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    assert_eq!(wire["body"]["tool_choice"], json!({"type":"none"}));
}

// Cached/indexed search modes cannot be expressed on the Messages wire; the
// request must fail before any bytes reach the provider instead of silently
// widening to live search.
#[tokio::test]
async fn cached_web_search_mode_fails_before_the_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let mut request = support::request(vec![support::user()]);
    support::set_tools(
        &mut request,
        json!([{"type":"web_search","external_web_access":false}]),
    );
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let error = match stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("cached mode must fail the request, not silently widen to live search"),
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("cached mode") && message.contains("live search mode"),
        "error must name the mode and the supported alternative: {message}"
    );
}

#[tokio::test]
async fn required_tool_choice_with_hosted_only_tools_fails_before_the_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let mut request = support::request(vec![support::user()]);
    request.tool_choice = "required".into();
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let error = match stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("forced tool choice without function tools must fail the request"),
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("required") && message.contains("function tool"),
        "error must name the choice and why it cannot be honored: {message}"
    );
}
