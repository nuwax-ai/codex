use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn call(id: &str) -> Value {
    json!({"type": "server_tool_use", "id": id, "name": "web_search", "input": {"query": "q"}})
}

fn result(id: &str) -> Value {
    json!({"type": "web_search_tool_result", "tool_use_id": id,
           "content": [{"type": "web_search_result", "encrypted_content": "ENC"}]})
}

#[test]
fn envelope_round_trips_and_bare_arrays_parse_as_legacy() {
    let value = envelope("Anthropic:abc", vec![call("s1"), result("s1")], Vec::new());
    let parsed = parse_envelope(&value).expect("v1 envelope");
    assert_eq!(parsed.source.as_deref(), Some("Anthropic:abc"));
    assert_eq!(parsed.blocks.len(), 2);

    let legacy = parse_envelope(&json!([call("s1")])).expect("legacy bare array");
    assert_eq!(legacy.source, None);

    assert!(parse_envelope(&json!({"version": 2, "source": "x", "blocks": []})).is_none());
    assert!(parse_envelope(&json!({"version": 1, "source": "x"})).is_none());
    assert!(parse_envelope(&json!("text")).is_none());
}

#[test]
fn replay_requires_an_exact_source_match() {
    let current = "Anthropic:abc";
    assert!(replayable(
        &parse_envelope(&envelope(current, vec![call("s1")], Vec::new())).expect("envelope"),
        current
    ));
    for other in [
        // Different endpoint/model identity.
        "Anthropic:def",
        // Different protocol label with the same hash.
        "Chat:abc",
        // Legacy payloads carry no source.
        "",
    ] {
        let payload = if other.is_empty() {
            json!([call("s1")])
        } else {
            envelope(other, vec![call("s1")], Vec::new())
        };
        assert!(
            !replayable(&parse_envelope(&payload).expect("envelope"), current),
            "{other} must not replay onto {current}"
        );
    }
}

fn groups(pairs: &[(usize, Value, Option<Value>)]) -> Vec<ReplayGroup> {
    pairs
        .iter()
        .map(|(index, call, result)| ReplayGroup {
            index: *index,
            blocks: match result {
                Some(result) => vec![call.clone(), result.clone()],
                None => vec![call.clone()],
            },
            cited_text: Vec::new(),
        })
        .collect()
}

fn sanitized_blocks(sanitized: &[ReplayGroup]) -> Vec<(usize, Vec<Value>)> {
    sanitized
        .iter()
        .map(|group| (group.index, group.blocks.clone()))
        .collect()
}

#[test]
fn sanitize_keeps_well_formed_pairs_and_merges_by_assistant_index() {
    let sanitized = sanitize_for_request(groups(&[
        (0, call("s1"), Some(result("s1"))),
        (0, call("s2"), None),
        (2, call("s3"), Some(result("s3"))),
    ]));
    assert_eq!(
        sanitized_blocks(&sanitized),
        vec![
            (0, vec![call("s1"), result("s1"), call("s2")]),
            (2, vec![call("s3"), result("s3")]),
        ]
    );
}

#[test]
fn sanitize_counts_real_pairs_not_assistant_groups() {
    // 65 pairs inside ONE assistant group: the phase-D group cap passed this;
    // the request-side cap must drop the oldest pair.
    let mut pairs = Vec::new();
    for i in 0..=MAX_REPLAY_PAIRS_PER_REQUEST {
        let id = format!("s{i}");
        pairs.push((0, call(&id), Some(result(&id))));
    }
    let sanitized = sanitize_for_request(groups(&pairs));
    assert_eq!(sanitized.len(), 1);
    assert_eq!(sanitized[0].blocks.len(), MAX_REPLAY_PAIRS_PER_REQUEST * 2);
    // The oldest pair (s0) dropped; the newest survived.
    assert!(!sanitized[0].blocks.contains(&call("s0")));
    assert!(sanitized[0].blocks.contains(&call("s64")));
}

#[test]
fn sanitize_drops_oversized_pairs_whole_and_keeps_the_rest() {
    let big = json!({"type": "web_search_tool_result", "tool_use_id": "big",
        "content": [{"type": "web_search_result", "encrypted_content": "x".repeat(MAX_PAIR_BYTES)}]});
    let sanitized = sanitize_for_request(groups(&[
        (0, call("ok"), Some(result("ok"))),
        (0, call("big"), Some(big)),
    ]));
    assert_eq!(
        sanitized_blocks(&sanitized),
        vec![(0, vec![call("ok"), result("ok")])],
        "the oversized pair drops whole — never truncated, never half-kept"
    );
}

#[test]
fn sanitize_drops_malformed_calls_and_dangling_results() {
    let nameless = json!({"type": "server_tool_use", "id": "x", "input": {}});
    let dangling = result("no-such-call");
    let sanitized = sanitize_for_request(groups(&[
        (0, nameless, None),
        (0, call("s1"), Some(result("s1"))),
    ]));
    assert_eq!(
        sanitized_blocks(&sanitized),
        vec![(0, vec![call("s1"), result("s1")])]
    );
    let only_dangling = sanitize_for_request(groups(&[(0, dangling, None)]));
    // A group with no surviving pair produces no group: no empty anchors,
    // no dangling halves.
    assert_eq!(
        sanitized_blocks(&only_dangling),
        Vec::<(usize, Vec<Value>)>::new()
    );
    let _ = dangling;
}
