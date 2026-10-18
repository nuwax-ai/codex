use super::support;
use codex_protocol::mcp::CallToolResult;
use codex_rust_rig_bridge::RigProtocol;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn structured_mcp_screenshot_keeps_metadata_and_typed_image_on_all_wires() {
    let metadata = json!({"width":640,"height":480}).to_string();
    for protocol in [
        RigProtocol::Chat,
        RigProtocol::Anthropic,
        RigProtocol::Responses,
    ] {
        let output = CallToolResult {
            content: vec![
                json!({"type":"text","text":"fallback must not be duplicated"}),
                json!({"type":"image","data":"aGVsbG8=","mimeType":"image/png","_meta":{"codex/imageDetail":"high"}}),
            ],
            structured_content: Some(json!({"width":640,"height":480})),
            is_error: Some(false),
            meta: None,
        }
        .as_function_call_output_payload();
        let mut request = support::request(vec![
            support::user(),
            json!({"type":"function_call","name":"screenshot","call_id":"shot","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"shot","output":output}),
        ]);
        support::set_tools(
            &mut request,
            json!([{"type":"function","name":"screenshot","parameters":{"type":"object","properties":{}}}]),
        );
        let (wire, _, _) = support::capture(&request, protocol).await;
        match protocol {
            RigProtocol::Responses => assert_eq!(
                wire["body"]["input"][2]["output"],
                json!([
                    {"type":"input_text","text":metadata},
                    {"type":"input_image","image_url":support::IMAGE,"detail":"high"}
                ])
            ),
            RigProtocol::Chat => {
                let messages = wire["body"]["messages"].as_array().unwrap();
                let index = messages
                    .iter()
                    .position(|message| message["role"] == "tool")
                    .unwrap();
                assert_eq!(messages[index]["tool_call_id"], "shot");
                let text = messages[index]["content"].as_str().unwrap();
                assert_eq!(text.matches(&metadata).count(), 1);
                assert!(!text.contains("fallback must not be duplicated"));
                assert!(!text.contains("aGVsbG8="));
                assert_eq!(
                    messages[index + 1]["content"][1]["image_url"],
                    json!({"url":support::IMAGE,"detail":"high"})
                );
            }
            RigProtocol::Anthropic => {
                let results: Vec<_> = wire["body"]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|message| message["content"].as_array())
                    .flatten()
                    .filter(|part| part["type"] == "tool_result" && part["tool_use_id"] == "shot")
                    .collect();
                assert_eq!(results.len(), 1);
                assert_eq!(
                    results[0]["content"],
                    json!([
                        {"type":"text","text":metadata},
                        {"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}}
                    ])
                );
            }
        }
    }
}
