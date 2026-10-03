//! Actual signed replay follows immutable credential bytes, not an env selector.
use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_actual_credentials_keep_signed_replay_and_rotation_drops_only_opaque()
-> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = MockServer::start().await;
    let thinking =
        json!({"type":"thinking", "thinking":"visible thought", "signature":"SIGNED_CREDENTIAL_A"});
    let signed_frame = format!(
        "event: content_block_start\ndata: {}\n\n",
        json!({"type":"content_block_start", "index":0, "content_block":thinking})
    );
    let cited = json!({"type":"text", "text":"visible answer", "citations":[{
        "type":"web_search_result_location", "url":"https://example.com", "title":"example",
        "cited_text":"finding", "encrypted_index":"INDEX_CREDENTIAL_A"}]});
    let mut cited_start = cited.clone();
    cited_start["text"] = json!("");
    let cited_frame = format!(
        "event: content_block_start\ndata: {}\n\n",
        json!({"type":"content_block_start", "index":3, "content_block":cited_start})
    );
    let cited_delta = format!(
        "event: content_block_delta\ndata: {}\n\n",
        json!({"type":"content_block_delta", "index":3, "delta":{"type":"text_delta", "text":"visible answer"}})
    );
    let first = format!(
        "{}{}{}{}{}{}{}{}{}{}{}",
        message_start("credentialA"),
        signed_frame,
        block_stop(0),
        server_tool_use_block(1, "srvu_credentials", "query"),
        block_stop(1),
        search_result_block(2, "srvu_credentials", "ENCRYPTED_CREDENTIAL_A"),
        block_stop(2),
        cited_frame,
        cited_delta,
        block_stop(3),
        message_delta("end_turn")
    );
    let follow = format!(
        "{}{}{}",
        message_start("follow"),
        plain_text_block(0, "followed"),
        message_delta("end_turn")
    );
    let responder = mount_anthropic_sequence(&server, vec![first, follow.clone(), follow]).await;
    let provider = ModelProviderInfo {
        requires_openai_auth: true,
        // The selector remains identical while the actual key rotates. PATH is
        // existing non-secret state; the test never changes process environment.
        env_http_headers: Some([("x-test-selector".into(), "PATH".into())].into()),
        ..anthropic_provider(&server.uri())
    };
    let initial = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("credential-a"))
        .with_config({
            let provider = provider.clone();
            move |config| config.model_provider = provider
        })
        .build_with_auto_env(&server)
        .await?;
    initial.submit_text_turn("search").await?;
    initial.submit_text_turn("follow up").await?;
    let rollout = initial
        .codex
        .rollout_path()
        .context("credential-bound rollout")?;
    initial.codex.shutdown_and_wait().await?;
    let before = std::fs::read_to_string(&rollout)?;
    let bodies = responder.captured();
    assert_eq!(bodies.len(), 2);
    let mut expected_content = vec![thinking];
    expected_content.extend(search_replay_blocks(
        "srvu_credentials",
        "query",
        "ENCRYPTED_CREDENTIAL_A",
    ));
    expected_content.push(cited);
    assert_eq!(
        bodies[1]["messages"],
        expected_follow_up_messages(&bodies[0]["messages"], expected_content, "follow up")
    );
    let sources: Vec<_> = before
        .lines()
        .map(codex_rollout::parse_rollout_line)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|line| match line.item {
            codex_history::RolloutItem::ResponseItem(item) => item
                .metadata
                .and_then(|metadata| metadata.model_output_provenance),
            _ => None,
        })
        .collect();
    assert!(
        sources
            .iter()
            .any(|source| source.auth_domain_kind.as_deref() == Some("credentialInstance"))
    );
    assert!(sources.iter().all(|source| {
        source
            .auth_domain
            .as_ref()
            .is_none_or(|domain| !domain.contains("credential-a"))
    }));
    let cwd = initial.config.cwd.clone();
    let resumed = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("credential-b"))
        .with_config(move |config| {
            config.model_provider = provider;
            config.cwd = cwd;
        })
        .resume(&server, initial.home.clone(), rollout.clone())
        .await?;
    resumed.submit_text_turn("after rotation").await?;
    resumed.codex.shutdown_and_wait().await?;
    let bodies = responder.captured();
    assert_eq!(bodies.len(), 3);
    let projected = serde_json::to_string(&bodies[2]["messages"])?;
    for opaque in [
        "SIGNED_CREDENTIAL_A",
        "ENCRYPTED_CREDENTIAL_A",
        "INDEX_CREDENTIAL_A",
    ] {
        assert!(
            !projected.contains(opaque),
            "old opaque data crossed credential rotation"
        );
    }
    let visible: Vec<_> = bodies[2]["messages"]
        .as_array()
        .context("rotated messages")?
        .iter()
        .filter(|message| {
            message["role"] == "assistant"
                && message["content"].as_array().is_some_and(|content| {
                    content
                        .iter()
                        .any(|block| block["text"] == "visible answer")
                })
        })
        .cloned()
        .collect();
    assert_eq!(
        visible,
        vec![json!({"role":"assistant", "content":[{"type":"text", "text":"visible answer"}]})]
    );
    assert!(
        std::fs::read_to_string(rollout)?.starts_with(&before),
        "rotation must append, not rewrite persisted history"
    );
    Ok(())
}
