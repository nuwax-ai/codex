//! Table tests for hosted tool translation.

use super::anthropic_server_tool;
use super::is_web_search_server_use;
use super::web_search_action;
use super::web_search_blocks_from_anthropic_sse;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn web_search_translates_to_the_anthropic_server_tool() {
    assert_eq!(
        anthropic_server_tool(&json!({"type": "web_search"})),
        Some(json!({"type": "web_search_20250305", "name": "web_search"}))
    );
}

#[test]
fn unknown_hosted_tools_have_no_translation() {
    assert_eq!(
        anthropic_server_tool(&json!({"type": "image_generation"})),
        None
    );
    assert_eq!(anthropic_server_tool(&json!({})), None);
}

#[test]
fn web_search_recognition_covers_official_and_gateway_tool_names() {
    assert!(is_web_search_server_use("web_search"));
    assert!(is_web_search_server_use("web_search_prime"));
    assert!(!is_web_search_server_use("bash_20250124"));
    assert!(!is_web_search_server_use(""));
}

#[test]
fn web_search_action_reads_official_and_gateway_query_fields() {
    assert_eq!(
        web_search_action(&json!({"query": "shanghai weather"})),
        codex_protocol::models::WebSearchAction::Search {
            query: Some("shanghai weather".into()),
            queries: None,
        }
    );
    assert_eq!(
        web_search_action(&json!({"search_query": "上海天气", "location": "cn"})),
        codex_protocol::models::WebSearchAction::Search {
            query: Some("上海天气".into()),
            queries: None,
        }
    );
    // No recognizable query field: the action still records a search, just
    // without a query, mirroring Responses' optional action payloads.
    assert_eq!(
        web_search_action(&json!({"location": "cn"})),
        codex_protocol::models::WebSearchAction::Search {
            query: None,
            queries: None,
        }
    );
}

#[test]
fn sse_parser_reads_inline_gateway_input_without_deltas() {
    // GLM inlines the complete input on content_block_start (live stream
    // 2026-09-28); Anthropic streams it via input_json_delta (covered by the
    // wire tests).
    let sse = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_inline\",\"name\":\"web_search_prime\",\"input\":{\"search_query\":\"上海天气\",\"location\":\"cn\"}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    );
    assert_eq!(
        web_search_blocks_from_anthropic_sse(sse.as_bytes()),
        vec![json!({
            "id": "srvu_inline",
            "name": "web_search_prime",
            "input": {"search_query": "上海天气", "location": "cn"},
        })]
    );
}

#[test]
fn sse_parser_skips_non_web_search_server_tools_and_malformed_frames() {
    let sse = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"b1\",\"name\":\"bash_20250124\",\"input\":{}}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "data: not-json\n\n",
    );
    assert_eq!(
        web_search_blocks_from_anthropic_sse(sse.as_bytes()),
        Vec::<serde_json::Value>::new()
    );
}
