use crate::hosted_replay::raw_assistant_content;
use crate::hosted_tools::web_search_blocks_from_anthropic_sse;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

fn frames(events: Vec<Value>) -> Vec<u8> {
    events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>()
        .into_bytes()
}

#[tokio::test]
async fn citation_deltas_preserve_inline_text_and_thinking_signatures() {
    let citation = json!({"type":"web_search_result_location","url":"https://example.com","title":"example","cited_text":"quote","encrypted_index":"index"});
    let bytes = frames(vec![
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"initial"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" text"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":citation}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"thinking","thinking":"inline thought","signature":"first"}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"signature_delta","signature":"second"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_stop"}),
    ]);
    let text = json!({"type":"text","text":"initial text","citations":[citation]});
    assert_eq!(
        raw_assistant_content(&bytes)
            .await
            .expect("complete content"),
        vec![
            text.clone(),
            json!({"type":"thinking","thinking":"inline thought","signature":"firstsecond"})
        ]
    );
    let capture = web_search_blocks_from_anthropic_sse(&bytes).await;
    assert_eq!(capture.text_blocks.len(), 1);
    assert_eq!(capture.text_blocks[0].index, 0);
    assert_eq!(capture.text_blocks[0].block, text);
}

#[tokio::test]
async fn paused_capture_rejects_missing_stops_unknown_deltas_and_invalid_input() {
    for events in [
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"partial"}}),
            json!({"type":"message_stop"}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"future_delta","payload":"unknown"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_stop"}),
        ],
        vec![
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call","name":"tool","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_stop"}),
        ],
    ] {
        assert!(raw_assistant_content(&frames(events)).await.is_err());
    }
}
