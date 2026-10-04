use super::*;
use crate::hosted_replay::BlockSite;
use crate::hosted_replay::SegmentSite;
use pretty_assertions::assert_eq;
use serde_json::json;

fn call(id: &str) -> Value {
    json!({"type":"server_tool_use","id":id,"name":"web_search","input":{}})
}
fn result(id: &str) -> Value {
    json!({"type":"web_search_tool_result","tool_use_id":id,"content":[]})
}
fn text(value: &str) -> Value {
    json!({"type":"text","text":value})
}
fn cited(value: &str) -> Value {
    json!({"type":"text","text":value,"citations":[{"type":"search_result_location","cited_text":"quote"}]})
}
fn owner(response: &str, ordinal: usize) -> String {
    crate::hosted_replay::segment_id(response, ordinal)
}
fn pair(index: u64) -> Value {
    json!({"kind":"pair","index":index})
}
fn raw(index: u64, owner: &str, block: Value) -> Value {
    json!({"kind":if block.get("citations").is_some() {"cited"} else {"text"},"index":index,"owner":owner,"block":block})
}
fn segment(response: &str, ordinal: usize, start: usize) -> SegmentSite {
    SegmentSite {
        id: owner(response, ordinal),
        start,
        len: 1,
    }
}
fn group(
    blocks: Vec<Value>,
    sites: Vec<(u32, u64)>,
    responses: Vec<ResponseSite>,
    layouts: Vec<Vec<Value>>,
) -> ReplayGroup {
    ReplayGroup {
        index: 0,
        blocks,
        block_sites: Some(
            sites
                .into_iter()
                .map(|(layout, wire)| BlockSite { layout, wire })
                .collect(),
        ),
        layouts,
        responses,
        cited_text: Vec::new(),
    }
}

#[test]
fn identical_text_from_earlier_text_only_pause_never_claims_later_citations() {
    let mut content = vec![text("same"), text("same")];
    let response = ResponseSite {
        id: "late".into(),
        anchor: 1,
        segments: vec![segment("late", 0, 1)],
    };
    let g = group(
        vec![call("s"), result("s")],
        vec![(0, 0), (0, 1)],
        vec![response],
        vec![vec![
            pair(0),
            pair(1),
            raw(2, &owner("late", 0), cited("same")),
        ]],
    );
    splice_identity(&mut content, &g);
    assert_eq!(
        content,
        vec![text("same"), call("s"), result("s"), cited("same")]
    );
}

#[test]
fn pair_only_pause_precedes_its_late_result_and_identical_later_text() {
    let mut content = vec![text("same")];
    let g = group(
        vec![call("s"), result("s")],
        vec![(0, 0), (1, 0)],
        vec![
            ResponseSite {
                id: "early".into(),
                anchor: 0,
                segments: Vec::new(),
            },
            ResponseSite {
                id: "late".into(),
                anchor: 0,
                segments: vec![segment("late", 0, 0)],
            },
        ],
        vec![
            vec![pair(0)],
            vec![pair(0), raw(1, &owner("late", 0), cited("same"))],
        ],
    );
    splice_identity(&mut content, &g);
    assert_eq!(content, vec![call("s"), result("s"), cited("same")]);
}

#[test]
fn missing_full_carrier_keeps_sibling_pairs_at_their_own_response_boundary() {
    let mut content = vec![text("early"), text("late")];
    let g = group(
        vec![call("a"), result("a"), call("b"), result("b")],
        vec![(0, 0), (0, 1), (1, 1), (1, 2)],
        vec![
            ResponseSite {
                id: "early".into(),
                anchor: 0,
                segments: vec![segment("early", 0, 0)],
            },
            ResponseSite {
                id: "late".into(),
                anchor: 1,
                segments: vec![segment("late", 0, 1)],
            },
        ],
        vec![
            vec![pair(0), pair(1), raw(2, &owner("early", 0), cited("early"))],
            Vec::new(),
        ],
    );
    splice_identity(&mut content, &g);
    assert_eq!(
        content,
        vec![
            call("a"),
            result("a"),
            cited("early"),
            text("late"),
            call("b"),
            result("b")
        ]
    );
}

#[test]
fn full_layout_keeps_thinking_signatures_and_client_tool_boundaries() {
    let thought = json!({"type":"thinking","thinking":"secret","signature":"signed"});
    let tool = json!({"type":"tool_use","id":"client","name":"lookup","input":{}});
    let mut content = vec![text("intro"), thought.clone(), tool.clone(), text("answer")];
    let response = ResponseSite {
        id: "r".into(),
        anchor: 0,
        segments: (0..4).map(|n| segment("r", n, n)).collect(),
    };
    let g = group(
        vec![call("s"), result("s")],
        vec![(0, 1), (0, 4)],
        vec![response],
        vec![vec![
            raw(0, &owner("r", 0), text("intro")),
            pair(1),
            json!({"kind":"segment","index":2,"owner":owner("r",1),"part":0}),
            json!({"kind":"segment","index":3,"owner":owner("r",2),"part":0}),
            pair(4),
            raw(5, &owner("r", 3), cited("answer")),
        ]],
    );
    splice_identity(&mut content, &g);
    assert_eq!(
        content,
        vec![
            text("intro"),
            call("s"),
            thought,
            tool,
            result("s"),
            cited("answer")
        ]
    );
}

#[test]
fn malformed_duplicate_missing_or_extra_indices_cannot_duplicate_or_lose_pairs() {
    for layout in [
        vec![pair(0), pair(0), pair(1)],
        vec![pair(0)],
        vec![pair(0), pair(1), pair(9)],
    ] {
        let mut content = vec![text("unchanged")];
        let g = group(
            vec![call("s"), result("s")],
            vec![(0, 0), (0, 1)],
            vec![ResponseSite {
                id: "r".into(),
                anchor: 1,
                segments: Vec::new(),
            }],
            vec![layout],
        );
        splice_identity(&mut content, &g);
        assert_eq!(content, vec![text("unchanged"), call("s"), result("s")]);
    }
}

#[test]
fn missing_named_owner_never_injects_text_and_edited_owned_text_stays_whole() {
    for segments in [Vec::new(), vec![segment("r", 0, 0)]] {
        let mut content = vec![text("edited")];
        let g = group(
            vec![call("s"), result("s")],
            vec![(0, 0), (0, 1)],
            vec![ResponseSite {
                id: "r".into(),
                anchor: 1,
                segments,
            }],
            vec![vec![
                pair(0),
                pair(1),
                raw(2, &owner("r", 0), cited("original")),
            ]],
        );
        splice_identity(&mut content, &g);
        assert_eq!(content, vec![text("edited"), call("s"), result("s")]);
    }
}

#[test]
fn successful_rebuild_keeps_unindexed_foreign_call_clones_with_their_result() {
    // A late response completes a search whose original call envelope was
    // dropped (for example over the envelope cap), so history carries a
    // foreign call clone with no wire index beside the new result. A
    // successful layout rebuild must keep both blocks: the replayed result
    // may never lose its matching server_tool_use.
    let mut content = vec![text("intro")];
    let response = ResponseSite {
        id: "late".into(),
        anchor: 0,
        segments: vec![segment("late", 0, 0)],
    };
    let g = group(
        vec![call("foreign"), result("foreign")],
        vec![(0, UNKNOWN_WIRE_INDEX), (0, 2)],
        vec![response],
        vec![vec![raw(1, &owner("late", 0), text("intro")), pair(2)]],
    );
    splice_identity(&mut content, &g);
    assert_eq!(
        content,
        vec![call("foreign"), text("intro"), result("foreign")]
    );
}
