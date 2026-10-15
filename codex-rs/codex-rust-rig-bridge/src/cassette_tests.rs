use super::*;
use pretty_assertions::assert_eq;
use rig_core::completion::message::ToolCall;
use rig_core::completion::request::FinishReason;
use rig_core::completion::request::Usage;
use rig_core::streaming::Item;
use rig_core::streaming::Part;
use rig_core::streaming::StreamEvent;

fn part_of(index: u32) -> Part {
    serde_json::from_value(serde_json::Value::from(index)).expect("Part is transparent")
}

fn terminal() -> TerminalRecord {
    TerminalRecord {
        finish_reason: Some(FinishReason::Stop),
        response_id: None,
        model: None,
        usage: Usage::new(),
    }
}

#[test]
fn replay_includes_created_and_stops_at_the_first_completion() {
    let recording = RigRecording {
        terminal: Some(terminal()),
        ..Default::default()
    };
    let events = replay_rig_events(&recording, HashSet::new()).unwrap();
    let expected = vec![
        ResponseEvent::Created { response_id: None },
        ResponseEvent::Completed {
            response_id: String::new(),
            token_usage: None,
            usage_metadata: None,
            end_turn: Some(true),
        },
    ];
    assert_eq!(
        serde_json::to_value(events).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn replay_rejects_missing_terminal_records() {
    let error = replay_rig_events(&RigRecording::default(), HashSet::new()).unwrap_err();
    assert!(
        matches!(error, codex_api::ApiError::Stream(message) if message.contains("without a terminal record"))
    );
}

#[test]
fn replay_does_not_recover_from_a_conversion_error_at_the_terminal() {
    // 0.44's `CallId` no longer deserializes from a bare wire string, so the
    // invalid call (custom tool whose arguments lack `input`) is built in
    // code; the replay contract under test is unchanged.
    let invalid = ToolCall::from_wire(
        "call-invalid",
        rig_core::completion::message::ToolFunction {
            name: rig_core::completion::message::ToolName::new("custom")
                .expect("non-empty tool name"),
            arguments: serde_json::json!({"missing_input": true})
                .as_object()
                .cloned()
                .unwrap_or_default(),
            invalid_arguments: None,
        },
    );
    let recording = RigRecording {
        events: vec![Item::Event(StreamEvent::End {
            part: part_of(1),
            content: rig_core::completion::message::AssistantContent::ToolCall(invalid),
        })],
        terminal: Some(terminal()),
    };
    let error = replay_rig_events(&recording, HashSet::from(["custom".into()])).unwrap_err();
    assert!(matches!(error, codex_api::ApiError::Stream(_)));
}

#[test]
fn legacy_anthropic_usage_upgrades_input_and_rederives_the_total() {
    // A recorded 0.42 Anthropic Final usage: `input_tokens` excluded cache and the
    // recorded `total_tokens` excluded it too. 0.44 replay upgrades both
    // exactly as the live pump's wire-counter correction would have
    // recorded them; the bridge_live fixture replays pin this end-to-end.
    let usage: LegacyUsage = serde_json::from_value(serde_json::json!({
        "input_tokens": 131,
        "output_tokens": 51,
        "total_tokens": 182,
        "cached_input_tokens": 128,
        "cache_creation_input_tokens": 0,
        "reasoning_tokens": 32,
    }))
    .expect("legacy usage");
    let upgraded = usage.into_usage(crate::RigProtocol::Anthropic);
    let mut expected = Usage::new();
    expected.input_tokens = Some(259);
    expected.cached_input_tokens = Some(128);
    expected.output_tokens = Some(51);
    expected.reasoning_tokens = Some(32);
    expected.total_tokens = Some(310);
    assert_eq!(upgraded, expected);
}

#[test]
fn legacy_chat_replay_keeps_cache_inside_the_recorded_prompt_total() {
    // These counters are from the 0.42 MiMo tool-turn cassette: Chat's
    // prompt_tokens already includes the 128 cached tokens.
    let fixture: RigEventFixture = serde_json::from_value(serde_json::json!({
        "vendor": "mimo",
        "tag": "cached-chat",
        "rig_events": [{
            "kind": "final",
            "provider": "openai",
            "response_id": "cached-response",
            "model": "mimo-v2.6-flash",
            "finish_reason": "stop",
            "usage": {
                "input_tokens": 131,
                "output_tokens": 51,
                "total_tokens": 182,
                "cached_input_tokens": 128,
                "cache_creation_input_tokens": 0,
                "reasoning_tokens": 32
            }
        }]
    }))
    .expect("legacy Chat fixture");
    let events = replay_fixture_events(&fixture).expect("replay legacy Chat usage");
    let expected = vec![
        ResponseEvent::Created { response_id: None },
        ResponseEvent::ServerModel("mimo-v2.6-flash".into()),
        ResponseEvent::Completed {
            response_id: "cached-response".into(),
            token_usage: Some(codex_protocol::protocol::TokenUsage {
                input_tokens: 131,
                cached_input_tokens: 128,
                output_tokens: 51,
                reasoning_output_tokens: 32,
                total_tokens: 182,
                ..Default::default()
            }),
            usage_metadata: None,
            end_turn: Some(true),
        },
    ];
    assert_eq!(
        serde_json::to_value(events).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn legacy_anthropic_replay_restores_message_id_and_inclusive_usage() {
    let fixture: RigEventFixture = serde_json::from_value(serde_json::json!({
        "vendor": "glm",
        "tag": "cached-anthropic",
        "rig_events": [{
            "kind": "final",
            "provider": "anthropic",
            "response_id": null,
            "message_id": "msg-legacy",
            "finish_reason": "stop",
            "usage": {
                "input_tokens": 60,
                "output_tokens": 23,
                "total_tokens": 211,
                "cached_input_tokens": 128,
                "cache_creation_input_tokens": 0
            }
        }]
    }))
    .expect("legacy Anthropic fixture");
    let events = replay_fixture_events(&fixture).expect("replay legacy Anthropic terminal");
    let expected = vec![
        ResponseEvent::Created { response_id: None },
        ResponseEvent::Completed {
            response_id: "msg-legacy".into(),
            token_usage: Some(codex_protocol::protocol::TokenUsage {
                input_tokens: 188,
                cached_input_tokens: 128,
                output_tokens: 23,
                total_tokens: 211,
                ..Default::default()
            }),
            usage_metadata: None,
            end_turn: Some(true),
        },
    ];
    assert_eq!(
        serde_json::to_value(events).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

#[test]
fn legacy_cassette_expands_reasoning_blocks_without_reusing_parts() {
    let events = vec![
        serde_json::json!({"id":"first", "reasoning":"alpha"}),
        serde_json::json!({"id":"first", "reasoning":{
            "id":"first",
            "content":[
                {"type":"text","content":{"text":"alpha","signature":"sig-a"}},
                {"type":"redacted","content":{"data":"redacted-data"}},
                {"type":"text","content":{"text":"beta","signature":"sig-b"}}
            ]
        }}),
        serde_json::json!({"id":"second", "reasoning":{
            "id":"second",
            "content":[{"type":"text","content":{"text":"next","signature":"sig-c"}}]
        }}),
    ];
    let recording = translate_legacy_events(&events, /*terminal*/ None)
        .expect("translate multipart legacy reasoning");
    let completed: Vec<_> = recording
        .events
        .into_iter()
        .filter_map(|event| match event {
            Item::Event(StreamEvent::End {
                part,
                content: rig_core::completion::message::AssistantContent::Reasoning(block),
            }) => Some((part.index(), block.text, block.redacted)),
            _ => None,
        })
        .collect();
    assert_eq!(
        completed,
        vec![
            (1, "alpha".into(), false),
            (2, String::new(), true),
            (3, "beta".into(), false),
            (4, "next".into(), false),
        ]
    );
}
