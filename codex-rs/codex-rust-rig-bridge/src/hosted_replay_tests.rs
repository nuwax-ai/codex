use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn call(id: &str) -> Value {
    json!({"type":"server_tool_use","id":id,"name":"web_search","input":{}})
}
fn result(id: &str) -> Value {
    json!({"type":"web_search_tool_result","tool_use_id":id,"content":[]})
}
fn layout(response: &str) -> Vec<Value> {
    vec![
        json!({"kind":"pair","index":0}),
        json!({"kind":"pair","index":1}),
        json!({"kind":"cited","index":2,"owner":segment_id(response,0),"block":{"type":"text","text":"answer","citations":[]}}),
    ]
}
fn group(index: usize, id: &str) -> ReplayGroup {
    ReplayGroup {
        index,
        blocks: vec![call(id), result(id)],
        block_sites: None,
        layouts: Vec::new(),
        responses: Vec::new(),
        cited_text: Vec::new(),
    }
}
fn identity(response: &str, id: &str) -> ReplayGroup {
    ReplayGroup {
        index: 0,
        blocks: vec![call(id), result(id)],
        block_sites: Some(vec![
            BlockSite { layout: 0, wire: 0 },
            BlockSite { layout: 0, wire: 1 },
        ]),
        layouts: vec![layout(response)],
        responses: vec![ResponseSite {
            id: response.into(),
            anchor: 0,
            segments: vec![SegmentSite {
                id: segment_id(response, 0),
                start: 0,
                len: 1,
            }],
        }],
        cited_text: Vec::new(),
    }
}

#[test]
fn v3_round_trip_owns_response_and_legacy_versions_never_upgrade_text_ownership() {
    let value = envelope(
        "source",
        vec![call("s"), result("s")],
        vec![0, 1],
        "r",
        Some(&layout("r")),
    );
    let parsed = validated_envelope(&value, "source").unwrap();
    assert_eq!(parsed.response_id.as_deref(), Some("r"));
    assert_eq!(parsed.layout, Some(layout("r")));
    for version in [1, 2] {
        let old = json!({"version":version,"source":"source","blocks":[call("s"),result("s")],
            "response_id":"r","block_indices":[0,1],"layout":layout("r")});
        let parsed = validated_envelope(&old, "source").unwrap();
        assert_eq!(
            (parsed.response_id, parsed.block_indices, parsed.layout),
            (None, None, None)
        );
    }
    assert!(validated_envelope(&json!([call("s")]), "source").is_none());
    assert!(validated_envelope(&value, "foreign").is_none());
    assert!(
        validated_envelope(
            &json!({"version":99,"source":"source","blocks":[call("s")]}),
            "source"
        )
        .is_none()
    );
}

#[test]
fn imported_identity_requires_unique_ordered_indices_and_same_response_segment_owner() {
    let base = envelope(
        "source",
        vec![call("s"), result("s")],
        vec![0, 1],
        "r",
        Some(&layout("r")),
    );
    for replacement in [
        json!([0, 0]),
        json!([1, 0]),
        json!([0]),
        json!([0, u64::MAX]),
    ] {
        let mut bad = base.clone();
        bad["block_indices"] = replacement;
        assert!(validated_envelope(&bad, "source").is_none(), "{bad}");
    }
    for bad_layout in [
        json!([{"kind":"pair","index":0},{"kind":"pair","index":0}]),
        json!([{"kind":"pair","index":2},{"kind":"pair","index":1}]),
        json!([{"kind":"cited","index":2,"owner":"rigseg_other_0","block":{"type":"text","text":"x","citations":[]}}]),
        json!([{"kind":"text","index":2,"owner":"rigseg_r_0","block":{"type":"thinking","text":"x"}}]),
        json!([{"kind":"segment","index":2,"owner":"rigseg_r_0","part":1}]),
    ] {
        let mut bad = base.clone();
        bad["layout"] = bad_layout;
        assert!(validated_envelope(&bad, "source").is_none(), "{bad}");
    }
}

#[test]
fn malformed_duplicate_and_dangling_pair_blocks_drop_whole() {
    for blocks in [
        vec![result("s")],
        vec![call("s"), call("s"), result("s")],
        vec![call("s"), result("s"), result("s")],
        vec![call("s"), result("other")],
        vec![json!({"type":"server_tool_use","id":"s","input":{}})],
    ] {
        let indices = (0..blocks.len() as u64).collect();
        assert!(
            validated_envelope(&envelope("source", blocks, indices, "r", None), "source").is_none()
        );
    }
}

#[test]
fn pair_caps_drop_complete_oldest_pairs_and_keep_split_halves_in_place() {
    let mut groups = Vec::new();
    for index in 0..=MAX_REPLAY_PAIRS_PER_REQUEST {
        let id = format!("s{index}");
        let mut first = group(index, &id);
        first.blocks.pop();
        groups.push(first);
        let mut late = group(index + 100, &id);
        late.blocks.remove(0);
        groups.push(late);
    }
    assert_eq!(sanitize_for_request(groups.clone()), groups[2..]);
    let mut big = group(0, "big");
    big.blocks[1]["content"] = json!([{"encrypted_content":"x".repeat(MAX_PAIR_BYTES)}]);
    let okay = group(1, "okay");
    assert_eq!(sanitize_for_request(vec![big, okay.clone()]), vec![okay]);
}

#[test]
fn layout_count_and_full_bytes_are_capped_independently_of_duplicate_pairs() {
    let mut g = identity("r0", "s0");
    for n in 1..200 {
        let response = format!("r{n}");
        // The duplicate pair contributes no accepted blocks but still has a
        // layout carrier. None of its text/layout metadata may survive dedupe.
        g.responses.push(ResponseSite {
            id: response.clone(),
            anchor: 0,
            segments: Vec::new(),
        });
        g.layouts.push(layout(&response));
        g.blocks.extend([call("s0"), result("s0")]);
        g.block_sites.as_mut().unwrap().extend([
            BlockSite { layout: n, wire: 0 },
            BlockSite { layout: n, wire: 1 },
        ]);
    }
    let sanitized = sanitize_for_request(vec![g]);
    assert_eq!(sanitized[0].blocks, vec![call("s0"), result("s0")]);
    assert_eq!(sanitized[0].responses.len(), 1);
    assert_eq!(sanitized[0].layouts.len(), 1);
    let mut groups: Vec<_> = (0..MAX_REPLAY_PAIRS_PER_REQUEST)
        .map(|n| identity(&format!("r{n}"), &format!("s{n}")))
        .collect();
    for group in &mut groups {
        group.layouts[0][2]["block"]["text"] = json!("x".repeat(2_000));
    }
    let sanitized = sanitize_for_request(groups);
    let layouts: Vec<_> = sanitized
        .iter()
        .flat_map(|group| &group.layouts)
        .filter(|layout| !layout.is_empty())
        .collect();
    assert!(layouts.len() <= MAX_LAYOUTS_PER_REQUEST);
    let bytes: usize = layouts
        .iter()
        .map(|layout| serde_json::to_vec(layout).unwrap().len())
        .sum();
    assert!(bytes <= MAX_LAYOUT_BYTES_PER_REQUEST);
    assert_eq!(
        sanitized[0].blocks.len(),
        MAX_REPLAY_PAIRS_PER_REQUEST * 2,
        "layout caps cannot erase pairs"
    );
}

#[test]
fn dropped_pair_entries_are_pruned_then_remaining_indices_have_full_coverage() {
    let mut g = identity("r", "s");
    let mut oversized_result = result("dropped");
    oversized_result["content"] = json!([{"encrypted_content":"x".repeat(MAX_PAIR_BYTES)}]);
    g.blocks.extend([call("dropped"), oversized_result]);
    g.block_sites.as_mut().expect("identity sites").extend([
        BlockSite { layout: 0, wire: 3 },
        BlockSite { layout: 0, wire: 4 },
    ]);
    g.layouts[0].extend([
        json!({"kind":"pair","index":3}),
        json!({"kind":"pair","index":4}),
    ]);
    // All original indices are valid, ordered and fully covered. The actual
    // pair byte budget removes one whole pair and only its layout entries.
    let mut expected = identity("r", "s");
    assert_eq!(sanitize_for_request(vec![g]), vec![expected.clone()]);

    // A missing reference for an accepted result cannot be repaired by the
    // pruning pass: identity degrades whole, but the accepted pair survives.
    let mut incomplete = identity("r", "s");
    incomplete.layouts[0].remove(1);
    expected.layouts[0].clear();
    assert_eq!(sanitize_for_request(vec![incomplete]), vec![expected]);
}
