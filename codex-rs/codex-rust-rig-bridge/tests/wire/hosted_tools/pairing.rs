use super::common::*;
use pretty_assertions::assert_eq;
use rig_core::completion::message::AssistantContent;
use rig_core::completion::message::Reasoning;

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
async fn anthropic_blank_assistant_keeps_search_replay_in_its_original_turn() {
    for text in ["", " \n\t "] {
        let first = search_pair("blank-turn");
        let second = search_pair("visible-turn");
        let messages = capture_search_replay(
            vec![
                support::user(),
                json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}),
                json!({"type":"web_search_call","wire_blocks":first}),
                support::user(),
                json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"later answer"}]}),
                json!({"type":"web_search_call","wire_blocks":second}),
                support::user(),
            ],
            SearchReplay::Enabled,
        )
        .await;
        let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
        let mut visible = vec![json!({"type":"text","text":"later answer"})];
        visible.extend(second.as_array().unwrap().iter().cloned());
        assert_eq!(
            messages,
            json!([
                user, {"role":"assistant","content":first},
                user, {"role":"assistant","content":visible}, user,
            ]),
            "blank assistant text {text:?} must not shift either replay group"
        );
    }
}

#[tokio::test]
async fn anthropic_reused_call_ids_keep_each_tool_failure_after_history_adaptation() {
    for [first_success, second_success] in [[false, false], [false, true], [true, false]] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let provider = provider(listener.local_addr().unwrap());
        let server =
            tokio::spawn(
                async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
            );
        let mut request = support::request(vec![
            support::user(),
            json!({"type":"function_call","name":"lookup","call_id":"c1","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"c1","output":"first result"}),
            support::user(),
            json!({"type":"function_call","name":"lookup","call_id":"c1","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"c1","output":"second result"}),
        ]);
        for (output, success) in request
            .input
            .iter_mut()
            .filter_map(|item| match item {
                ResponseItem::FunctionCallOutput { output, .. } => Some(output),
                _ => None,
            })
            .zip([first_success, second_success])
        {
            output.success = Some(success);
        }
        support::set_tools(
            &mut request,
            json!([{"type":"function","name":"lookup","parameters":{"type":"object","properties":{}}}]),
        );
        let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
        let mut stream = stream_via_rig(
            &request,
            &provider,
            &auth,
            http::HeaderMap::new(),
            RigProtocol::Anthropic,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
        let wire = server.await.unwrap();
        let mut expected = json!([
            {"role":"user","content":[{"type":"text","text":"hello"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"c1","name":"lookup","input":{}}]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"c1","content":[{"type":"text","text":"first result"}]},
                {"type":"text","text":"hello"},
            ]},
            {"role":"assistant","content":[{"type":"tool_use","id":"c2","name":"lookup","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"c2","content":[{"type":"text","text":"second result"}]}]},
        ]);
        if !first_success {
            expected[2]["content"][0]["is_error"] = true.into();
        }
        if !second_success {
            expected[4]["content"][0]["is_error"] = true.into();
        }
        assert_eq!(
            wire["body"]["messages"], expected,
            "tool outcomes must follow their adapted calls: {first_success}, {second_success}"
        );
    }
}

#[tokio::test]
async fn anthropic_edited_v2_reasoning_does_not_revalidate_its_stale_signature() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = provider(listener.local_addr().unwrap());
    let server =
        tokio::spawn(
            async move { support::serve_payload(&listener, support::ANTHROPIC_SSE).await },
        );
    let source = support::source_for_dummy_auth(&provider, RigProtocol::Anthropic);
    let AssistantContent::Reasoning(mut reasoning) =
        AssistantContent::Reasoning(Reasoning::new("original thought")).with_native(json!({
            "type":"thinking","thinking":"original thought","signature":"original-signature"
        }))
    else {
        panic!("reasoning block must keep its canonical kind");
    };
    reasoning.text = "edited thought".into();
    let envelope = json!({"source":source,"blocks":[reasoning]});
    let request = support::request(vec![
        support::user(),
        json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"edited thought"}],
            "encrypted_content":format!("codex-rig-reasoning-v2:{envelope}")}),
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}),
        support::user(),
    ]);
    let auth: SharedAuthProvider = Arc::new(support::DummyAuth);
    let mut stream = stream_via_rig(
        &request,
        &provider,
        &auth,
        http::HeaderMap::new(),
        RigProtocol::Anthropic,
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    while let Some(event) = stream.next().await {
        event.unwrap();
    }
    let wire = server.await.unwrap();
    assert_eq!(
        wire["body"]["messages"][1],
        json!({"role":"assistant","content":[
            {"type":"text","text":"edited thought"},
            {"type":"text","text":"answer"},
        ]})
    );
}

#[tokio::test]
async fn anthropic_search_only_after_tool_result_replays_in_its_own_assistant() {
    let blocks = search_pair("search-after-tool");
    let messages = capture_search_replay_with_tools(
        vec![
            support::user(),
            json!({"type":"function_call","name":"lookup","call_id":"call-1","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"call-1","output":"result"}),
            json!({"type":"web_search_call","wire_blocks":blocks}),
            support::user(),
        ],
        SearchReplay::Enabled,
        Some(json!([{"type":"function","name":"lookup","description":"lookup","parameters":{"type":"object","properties":{}}}])),
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
    // rig 0.44's history adapter merges adjacent user messages once the
    // anchor turns are gone; with no anchors to keep apart, the opt-out
    // body is one merged user turn (fork-contract adaptation).
    assert_eq!(
        messages,
        json!([{"role":"user","content":[
            {"type":"text","text":"hello"},
            {"type":"text","text":"hello"},
            {"type":"text","text":"hello"},
        ]}])
    );
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
        // Anchor-less drops leave adjacent user turns, which rig 0.44's
        // history adapter merges (fork-contract adaptation).
        assert_eq!(
            messages,
            json!([{"role":"user", "content":[
                {"type":"text", "text":"hello"},
                {"type":"text", "text":"hello"},
            ]}])
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
        // rig 0.44 merges the anchor-less adjacent user turns
        // (fork-contract adaptation).
        assert_eq!(
            capture_search_replay(items, SearchReplay::Disabled).await,
            json!([{"role":"user", "content":[
                {"type":"text", "text":"hello"},
                {"type":"text", "text":"hello"},
                {"type":"text", "text":"hello"},
            ]}])
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
    // The deduped tail turns hold no anchors, so rig 0.44 merges those
    // adjacent user turns (fork-contract adaptation).
    assert_eq!(
        messages,
        json!([
            user, {"role":"assistant", "content":[pair[0].clone()]},
            user, {"role":"assistant", "content":[pair[1].clone()]},
            {"role":"user", "content":[
                {"type":"text", "text":"hello"},
                {"type":"text", "text":"hello"},
                {"type":"text", "text":"hello"},
            ]},
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
