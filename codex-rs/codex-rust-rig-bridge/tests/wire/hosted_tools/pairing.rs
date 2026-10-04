use super::common::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn anthropic_two_search_only_turns_replay_exact_raw_groups() {
    let first = search_pair("search-first");
    let second = search_pair("search-second");
    let messages = capture_search_replay(
        vec![
            support::user(),
            json!({"type":"web_search_call","wire_blocks":first}),
            support::user(),
            json!({"type":"web_search_call","wire_blocks":second}),
            support::user(),
        ],
        SearchReplay::Enabled,
    )
    .await;
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(
        messages,
        json!([
            user, {"role":"assistant","content":first},
            user, {"role":"assistant","content":second}, user,
        ])
    );
}

#[tokio::test]
async fn anthropic_search_only_after_tool_result_replays_in_its_own_assistant() {
    let blocks = search_pair("search-after-tool");
    let messages = capture_search_replay(
        vec![
            support::user(),
            json!({"type":"function_call","name":"lookup","call_id":"call-1","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"call-1","output":"result"}),
            json!({"type":"web_search_call","wire_blocks":blocks}),
            support::user(),
        ],
        SearchReplay::Enabled,
    )
    .await;
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(
        messages,
        json!([
            user,
            {"role":"assistant","content":[{"type":"tool_use","id":"call-1","name":"lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":[{"type":"text","text":"result"}]}]},
            {"role":"assistant","content":blocks}, user,
        ])
    );
}

#[tokio::test]
async fn anthropic_search_only_opt_out_removes_sdk_anchors_and_empty_assistants() {
    let messages = capture_search_replay(vec![
        support::user(),
        json!({"type":"web_search_call","wire_blocks":search_pair("search-first")}),
        support::user(),
        json!({"type":"web_search_call","wire_blocks":search_pair("search-second")}),
        support::user(),
        json!({"type":"web_search_call","wire_blocks":[{"type":"unknown_legacy_block","opaque":"unused"}]}),
    ], SearchReplay::Disabled).await;
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(messages, json!([user, user, user]));
}

#[tokio::test]
async fn anthropic_replay_group_cap_removes_dropped_search_only_anchors() {
    let mut items = vec![support::user()];
    for index in 0..65 {
        items.push(
            json!({"type":"web_search_call","wire_blocks":search_pair(&format!("search-{index}"))}),
        );
        items.push(support::user());
    }
    let messages = capture_search_replay(items, SearchReplay::Enabled).await;
    let assistants = messages
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| message["content"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        assistants,
        (1..65)
            .map(|index| search_pair(&format!("search-{index}")))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn anthropic_search_dedupe_keeps_first_valid_same_source_completed_item() {
    let first = search_pair("same-id");
    let mut later = first.clone();
    later[1]["content"][0]["encrypted_content"] = json!("LATER");
    let pending = json!([first[0]]);
    let foreign = json!({"version":1, "source":"another-endpoint", "blocks":later});
    let oversized = json!({"version":1, "source":"test-current", "blocks":later,
        "cited_text":[{"type":"text", "text":"x".repeat(40_960), "citations":[]}]});
    for (payloads, expected) in [
        (vec![first.clone(), later], first.clone()),
        (vec![foreign.clone(), first.clone()], first.clone()),
        (vec![pending.clone(), foreign], pending),
        (vec![oversized, first.clone()], first),
    ] {
        let mut items = vec![support::user()];
        items.extend(
            payloads
                .into_iter()
                .map(|payload| json!({"type":"web_search_call", "wire_blocks":payload})),
        );
        items.push(support::user());
        let messages = capture_search_replay(items, SearchReplay::Enabled).await;
        let assistants: Vec<Value> = messages
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "assistant")
            .map(|message| message["content"].clone())
            .collect();
        assert_eq!(assistants, vec![expected]);
    }
}

#[tokio::test]
async fn anthropic_malformed_loaded_search_payloads_drop_before_sdk_conversion() {
    let call = search_pair("loaded")[0].clone();
    for payload in [
        json!({"version":1, "source":"test-current", "blocks":[{"type":"server_tool_use", "id":"loaded", "input":{}}]}),
        json!({"version":1, "source":"test-current", "blocks":[{"type":"server_tool_use", "id":"loaded", "name":"web_search", "input":false}]}),
        json!({"version":1, "source":"test-current", "blocks":[call.clone(), {"type":"web_search_tool_result", "tool_use_id":"loaded", "content":false}]}),
        json!({"version":1, "source":"test-current", "blocks":[call.clone()], "cited_text":[{"type":"tool_use", "text":"x", "citations":[]}]}),
        json!({"version":1, "source":"test-current", "blocks":[call], "cited_text":[{"type":"text", "text":"x", "citations":{}}]}),
    ] {
        let messages = capture_search_replay(
            vec![
                support::user(),
                json!({"type":"web_search_call", "wire_blocks":payload}),
                support::user(),
            ],
            SearchReplay::Enabled,
        )
        .await;
        assert_eq!(
            messages,
            json!([
                {"role":"user", "content":[{"type":"text", "text":"hello"}]},
                {"role":"user", "content":[{"type":"text", "text":"hello"}]},
            ])
        );
    }
}

#[tokio::test]
async fn anthropic_late_search_result_opt_out_removes_result_only_anchors() {
    for result_type in ["web_search_tool_result", "tool_result"] {
        let mut pair = search_pair("disabled-late");
        pair[1]["type"] = json!(result_type);
        let items = vec![
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":pair}),
            support::user(),
        ];
        let user = json!({"role":"user", "content":[{"type":"text", "text":"hello"}]});
        assert_eq!(
            capture_search_replay(items, SearchReplay::Disabled).await,
            json!([user, user, user])
        );
    }
}

#[tokio::test]
async fn anthropic_search_dedupe_across_user_messages_keeps_one_call_and_result() {
    let pair = search_pair("across-users");
    let mut duplicate = pair.clone();
    duplicate[1]["vendor_result"] = json!("must not replay");
    let messages = capture_search_replay(
        vec![
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":pair}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":duplicate}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
        ],
        SearchReplay::Enabled,
    )
    .await;
    let user = json!({"role":"user", "content":[{"type":"text", "text":"hello"}]});
    assert_eq!(
        messages,
        json!([
            user, {"role":"assistant", "content":[pair[0].clone()]},
            user, {"role":"assistant", "content":[pair[1].clone()]}, user, user, user,
        ])
    );
}

#[tokio::test]
async fn anthropic_split_search_pair_cap_drops_both_wire_positions() {
    let mut items = vec![support::user()];
    for index in 0..65 {
        let pair = search_pair(&format!("split-{index}"));
        items.extend([
            json!({"type":"web_search_call", "wire_blocks":[pair[0].clone()]}),
            support::user(),
            json!({"type":"web_search_call", "wire_blocks":pair}),
            support::user(),
        ]);
    }
    let messages = capture_search_replay(items, SearchReplay::Enabled).await;
    let assistants: Vec<Value> = messages
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| message["content"].clone())
        .collect();
    let expected: Vec<Value> = (1..65)
        .flat_map(|index| {
            let pair = search_pair(&format!("split-{index}"));
            [json!([pair[0].clone()]), json!([pair[1].clone()])]
        })
        .collect();
    assert_eq!(assistants, expected);
}
