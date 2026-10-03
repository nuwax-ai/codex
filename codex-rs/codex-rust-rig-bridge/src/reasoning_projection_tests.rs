use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn projected_chat_reasoning_keeps_complete_visible_content_without_opaque_bytes() {
    let item: ResponseItem = serde_json::from_value(json!({
        "type":"reasoning", "summary":[], "encrypted_content":null,
        "content":[{"type":"reasoning_text", "text":"first thought"},
            {"type":"reasoning_text", "text":"; second thought"}]
    }))
    .unwrap();
    assert_eq!(
        replay_reasoning(&item, "current-source", crate::RigProtocol::Chat),
        vec![Reasoning::new("first thought; second thought")]
    );
}

#[test]
fn projected_plaintext_budget_keeps_whole_content_at_the_boundary() {
    for (bytes, accepted) in [
        (crate::hosted_replay::MAX_PAIR_BYTES, true),
        (crate::hosted_replay::MAX_PAIR_BYTES + 1, false),
    ] {
        let text = "x".repeat(bytes);
        let item: ResponseItem = serde_json::from_value(json!({"type":"reasoning", "summary":[],
            "content":[{"type":"reasoning_text", "text":text}]}))
        .unwrap();
        let expected = if accepted {
            vec![Reasoning::new(&text)]
        } else {
            Vec::new()
        };
        assert_eq!(
            replay_reasoning(&item, "current-source", crate::RigProtocol::Chat),
            expected
        );
    }
}
