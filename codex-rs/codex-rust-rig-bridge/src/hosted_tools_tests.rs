//! Table tests for hosted tool translation.

use super::PairedWebSearchBlocks;
use super::anthropic_server_tool;
use super::is_web_search_server_use;
use super::pair_web_search_blocks;
use super::translate_anthropic_server_tools;
use super::web_search_action;
use super::web_search_blocks_from_anthropic_sse;
use super::web_search_call_events;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn web_search_translates_to_the_anthropic_server_tool() {
    assert_eq!(
        anthropic_server_tool(&json!({"type": "web_search"})),
        Ok(Some(
            json!({"type": "web_search_20250305", "name": "web_search"})
        ))
    );
}

#[test]
fn unknown_hosted_tools_have_no_translation() {
    assert_eq!(
        anthropic_server_tool(&json!({"type": "image_generation"})),
        Ok(None)
    );
    assert_eq!(anthropic_server_tool(&json!({})), Ok(None));
}

#[test]
fn allowed_domains_and_user_location_cross_the_translation() {
    assert_eq!(
        anthropic_server_tool(&json!({
            "type": "web_search",
            "external_web_access": true,
            "filters": {"allowed_domains": ["docs.rs", "example.com/blog"]},
            "user_location": {
                "type": "approximate",
                "city": "Shanghai",
                "country": "CN",
                "timezone": "Asia/Shanghai",
            },
        })),
        Ok(Some(json!({
            "type": "web_search_20250305",
            "name": "web_search",
            "allowed_domains": ["docs.rs", "example.com/blog"],
            "user_location": {
                "type": "approximate",
                "city": "Shanghai",
                "country": "CN",
                "timezone": "Asia/Shanghai",
            },
        })))
    );
}

#[test]
fn cached_mode_is_rejected_before_anything_is_sent() {
    let error = anthropic_server_tool(&json!({
        "type": "web_search",
        "external_web_access": false,
    }))
    .expect_err("cached search must fail the request");
    assert!(
        error.contains("cached mode") && error.contains("live search mode"),
        "error must name the mode and the supported alternative: {error}"
    );
}

#[test]
fn indexed_mode_is_rejected_before_anything_is_sent() {
    let error = anthropic_server_tool(&json!({
        "type": "web_search",
        "indexed_web_access": true,
    }))
    .expect_err("indexed search must fail the request");
    assert!(
        error.contains("indexed mode") && error.contains("live search mode"),
        "error must name the mode and the supported alternative: {error}"
    );
}

#[test]
fn malformed_domain_entries_are_named_in_the_error() {
    for (domains, expected) in [
        (json!(["https://docs.rs"]), "scheme"),
        (json!(["*.example.com"]), "wildcards in the domain"),
        (json!([""]), "must not be empty"),
        (json!([]), "must not be empty"),
        (json!([42]), "must be strings"),
    ] {
        let error = anthropic_server_tool(&json!({
            "type": "web_search",
            "filters": {"allowed_domains": domains},
        }))
        .expect_err("malformed allowed_domains must fail the request");
        assert!(
            error.contains(expected),
            "expected {expected:?} in {error:?} for {domains}"
        );
    }
}

#[test]
fn user_location_without_any_field_is_rejected() {
    let error = anthropic_server_tool(&json!({
        "type": "web_search",
        "user_location": {"type": "approximate"},
    }))
    .expect_err("an all-empty user_location must fail the request");
    assert!(
        error.contains("city, region, country or timezone"),
        "{error}"
    );
}

#[test]
fn batch_translation_fails_fast_on_the_first_bad_declaration() {
    let error = translate_anthropic_server_tools(&[
        json!({"type": "web_search"}),
        json!({"type": "web_search", "external_web_access": false}),
    ])
    .expect_err("one non-representable mode must abort the batch");
    assert!(error.to_string().contains("cached mode"));
}

#[test]
fn batch_translation_drops_unknown_tools_and_keeps_web_search() {
    let translated = translate_anthropic_server_tools(&[
        json!({"type": "web_search"}),
        json!({"type": "image_generation"}),
    ])
    .expect("unknown hosted tools drop with a warning");
    assert_eq!(
        translated,
        vec![json!({"type": "web_search_20250305", "name": "web_search"})]
    );
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
    let capture = futures::executor::block_on(web_search_blocks_from_anthropic_sse(sse.as_bytes()));
    let pairs = pair_web_search_blocks(capture).pairs;
    assert_eq!(
        serde_json::to_value(pairs.iter().map(|pair| &pair.call).collect::<Vec<_>>())
            .expect("encode"),
        json!([{
            "type": "server_tool_use",
            "id": "srvu_inline",
            "name": "web_search_prime",
            "input": {"search_query": "上海天气", "location": "cn"},
        }])
    );
    assert!(pairs[0].result.is_none(), "no result block in this fixture");
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
    let capture = futures::executor::block_on(web_search_blocks_from_anthropic_sse(sse.as_bytes()));
    assert!(pair_web_search_blocks(capture).pairs.is_empty());
}

// D1: result blocks pair with their calls by id, in original call order.
#[test]
fn official_result_blocks_pair_by_id() {
    let sse = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvu_1\",\"name\":\"web_search\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"q1\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvu_1\",\"content\":[{\"type\":\"web_search_result\",\"url\":\"https://example.com\",\"encrypted_content\":\"ENC\"}]}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    );
    let capture = futures::executor::block_on(web_search_blocks_from_anthropic_sse(sse.as_bytes()));
    let pairs = pair_web_search_blocks(capture).pairs;
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        pairs[0].result.as_ref().expect("paired result")["tool_use_id"],
        json!("srvu_1")
    );
    // The emitted item carries both raw blocks and a completed status.
    let events = web_search_call_events(
        PairedWebSearchBlocks {
            call: pairs[0].call.clone(),
            result: pairs[0].result.clone(),
        },
        "test-source",
    );
    let done = events
        .iter()
        .find_map(|event| match event {
            codex_api::ResponseEvent::OutputItemDone(item) => Some(item.clone()),
            _ => None,
        })
        .expect("done item");
    let encoded = serde_json::to_value(&done).expect("encode");
    assert_eq!(encoded["status"], json!("completed"));
    assert_eq!(
        encoded["wire_blocks"],
        json!({
            "version": 1,
            "source": "test-source",
            "blocks": [
                {"type": "server_tool_use", "id": "srvu_1", "name": "web_search",
                 "input": {"query": "q1"}},
                {"type": "web_search_tool_result", "tool_use_id": "srvu_1",
                 "content": [{"type": "web_search_result", "url": "https://example.com",
                              "encrypted_content": "ENC"}]},
            ],
        }),
        "the raw wire pair must ride the item verbatim in the versioned envelope"
    );
}

// D1: an unpaired call (mixed server/client turn) emits in_progress with
// only the use block.
#[test]
fn unpaired_call_emits_in_progress_with_use_block_only() {
    let events = web_search_call_events(
        PairedWebSearchBlocks {
            call: json!({"type": "server_tool_use", "id": "srvu_2", "name": "web_search",
                         "input": {"query": "mixed turn query"}}),
            result: None,
        },
        "test-source",
    );
    let done = events
        .iter()
        .find_map(|event| match event {
            codex_api::ResponseEvent::OutputItemDone(item) => Some(item.clone()),
            _ => None,
        })
        .expect("done item");
    let encoded = serde_json::to_value(&done).expect("encode");
    assert_eq!(encoded["status"], json!("in_progress"));
    assert_eq!(
        encoded["wire_blocks"],
        json!({
            "version": 1,
            "source": "test-source",
            "blocks": [{"type": "server_tool_use", "id": "srvu_2", "name": "web_search",
                        "input": {"query": "mixed turn query"}}],
        })
    );
}
