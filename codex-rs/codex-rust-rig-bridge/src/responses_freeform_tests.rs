use super::downgrade_custom_tools;
use super::parse_freeform_tools_flag;
use codex_api::ResponsesApiRequest;
use serde_json::Value;
use serde_json::json;

fn request_with_tools(tools: Value) -> ResponsesApiRequest {
    let raw = serde_json::value::to_raw_value(&tools).expect("tools json");
    ResponsesApiRequest {
        model: "mimo-v2.6-flash".into(),
        stream: true,
        service_tier: None,
        input: Vec::new(),
        tools: Some(codex_api::ResponsesApiTools::from(std::sync::Arc::from(
            raw,
        ))),
        tool_choice: "auto".into(),
        parallel_tool_calls: true,
        max_output_tokens: None,
        reasoning: None,
        store: false,
        stream_options: None,
        include: Vec::new(),
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
    }
}

fn tools_of(request: &ResponsesApiRequest) -> Value {
    let tools = request.tools.as_ref().expect("tools");
    serde_json::to_value(tools).expect("serialize tools")
}

/// The gateway-facing shape: MiMo (among others) rejects `type: "custom"` on
/// `/v1/responses`, so the default passthrough must hand it a function tool
/// whose payload contract survives — one `input` string plus the grammar.
#[test]
fn custom_tools_become_functions_with_the_input_wrapper() {
    let mut request = request_with_tools(json!([{
        "type": "custom",
        "name": "apply_patch",
        "description": "The `apply_patch` tool can be used to edit files.",
        "format": {
            "type": "grammar",
            "syntax": "lark",
            "definition": "start: begin_patch hunk+ end_patch"
        }
    }]));

    downgrade_custom_tools(&mut request).expect("downgrade");

    let tools = tools_of(&request);
    let tool = &tools[0];
    assert_eq!(tool["type"], json!("function"));
    assert_eq!(tool["name"], json!("apply_patch"));
    assert_eq!(
        tool["parameters"],
        json!({
            "type": "object",
            "properties": {"input": {"type": "string", "description": "The complete tool input text"}},
            "required": ["input"],
            "additionalProperties": false
        })
    );
    let description = tool["description"].as_str().expect("description");
    // The grammar can only ride in the description once the format channel is
    // gone; the wrapper must say so explicitly or the model wraps the patch in
    // a second layer of JSON.
    assert!(description.contains("outer tool arguments are JSON with a single `input` string"));
    assert!(description.contains("start: begin_patch hunk+ end_patch"));
    assert!(tool.get("format").is_none());
}

/// Function and hosted declarations are not the bridge's business: they must
/// keep their exact shape (schema, strictness, server-side fields).
#[test]
fn other_tool_declarations_are_left_untouched() {
    let function = json!({
        "type": "function",
        "name": "shell",
        "description": "Run a command",
        "parameters": {"type": "object", "properties": {"command": {"type": "string"}}},
        "strict": false
    });
    let hosted = json!({"type": "web_search", "external_web_access": true});
    let mut request = request_with_tools(json!([function.clone(), hosted.clone()]));

    downgrade_custom_tools(&mut request).expect("downgrade");

    assert_eq!(tools_of(&request), json!([function, hosted]));
}

/// Order and the untouched neighbours survive a mixed declaration list.
#[test]
fn only_custom_declarations_are_rewritten_in_place() {
    let mut request = request_with_tools(json!([
        {"type": "function", "name": "shell", "parameters": {"type": "object"}},
        {"type": "custom", "name": "apply_patch", "description": "patch"},
        {"type": "function", "name": "view_image", "parameters": {"type": "object"}}
    ]));

    downgrade_custom_tools(&mut request).expect("downgrade");

    let tools = tools_of(&request);
    let names: Vec<_> = tools
        .as_array()
        .expect("array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["shell", "apply_patch", "view_image"]);
    assert_eq!(tools[0]["type"], json!("function"));
    assert_eq!(tools[1]["type"], json!("function"));
    assert_eq!(tools[2]["type"], json!("function"));
    // No grammar declared: the declaration's own description rides through
    // untouched, exactly like the Chat wire's wrapper.
    assert_eq!(tools[1]["description"], json!("patch"));
}

/// A request without tools, or with no custom declaration, is left alone.
#[test]
fn requests_without_custom_tools_keep_their_declarations() {
    let mut request = request_with_tools(json!([]));
    downgrade_custom_tools(&mut request).expect("downgrade");
    assert_eq!(tools_of(&request), json!([]));

    let mut request = request_with_tools(json!([]));
    request.tools = None;
    downgrade_custom_tools(&mut request).expect("downgrade");
    assert!(request.tools.is_none());
}

/// The escape hatch keeps the verbatim custom declaration for gateways that do
/// implement the Responses tool model.
#[test]
fn the_flag_parses_the_documented_spellings() {
    for enabled in ["1", "true", "TRUE", "yes", "on", " 1 "] {
        assert!(
            parse_freeform_tools_flag(enabled).expect(enabled),
            "{enabled}"
        );
    }
    for disabled in ["0", "false", "no", "off"] {
        assert!(
            !parse_freeform_tools_flag(disabled).expect(disabled),
            "{disabled}"
        );
    }
    let error = parse_freeform_tools_flag("maybe").expect_err("invalid value");
    assert!(error.contains("is not a boolean"), "{error}");
}

/// A thread that recorded free-form calls (the Chat wire keeps the custom
/// declaration, and `NUWAX_FREEFORM_TOOLS=1` keeps it here) must not replay
/// them to a gateway the request just told that every tool is a function — and
/// the converted result has to stay a tool result so the trailing-result
/// repair above still recognizes it.
#[test]
fn custom_tool_history_is_rewritten_as_function_history() {
    let mut request = request_with_tools(json!([{
        "type": "custom",
        "name": "apply_patch",
        "description": "freeform patch tool",
        "format": { "type": "grammar", "syntax": "lark", "definition": "start: patch" }
    }]));
    request.input = serde_json::from_value(json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
        {"type":"custom_tool_call","name":"apply_patch","call_id":"call-1","input":"*** Begin Patch"},
        {"type":"custom_tool_call_output","call_id":"call-1","output":"ok"}
    ]))
    .expect("input items");

    super::prepare_responses_input(
        &mut request,
        /*freeform_tools*/ false,
        /*repair*/ true,
    )
    .expect("prepare");

    assert_eq!(tools_of(&request)[0]["type"], json!("function"));
    let input = serde_json::to_value(&request.input).expect("serialize input");
    let items = input.as_array().expect("input array");
    // The converted result is still a tool result, so the trailing-result
    // repair recognizes it and closes the request with its user turn.
    assert_eq!(
        &items[..3],
        json!([
            {"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]},
            {
                "type":"function_call",
                "name":"apply_patch",
                "arguments":"{\"input\":\"*** Begin Patch\"}",
                "call_id":"call-1"
            },
            {"type":"function_call_output","call_id":"call-1","output":"ok"}
        ])
        .as_array()
        .expect("expected items")
        .as_slice()
    );
    assert_eq!(items.len(), 4, "trailing tool result got its user turn");
    assert_eq!(items[3]["type"], json!("message"));
    assert_eq!(items[3]["role"], json!("user"));
}

/// The rewrite is a no-op on a request that carries no custom history, so a
/// function-only thread keeps its bytes.
#[test]
fn function_only_history_is_left_alone() {
    let mut request = request_with_tools(json!([{
        "type": "function",
        "name": "shell",
        "parameters": {"type":"object","properties":{"command":{"type":"string"}}}
    }]));
    request.input = serde_json::from_value(json!([
        {"type":"function_call","name":"shell","arguments":"{\"command\":\"ls\"}","call_id":"call-1"},
        {"type":"function_call_output","call_id":"call-1","output":"ok"}
    ]))
    .expect("input items");
    let before = serde_json::to_value(&request.input).expect("serialize input");

    assert_eq!(super::downgrade_custom_tool_history(&mut request.input), 0);
    assert_eq!(
        serde_json::to_value(&request.input).expect("serialize input"),
        before
    );
}
