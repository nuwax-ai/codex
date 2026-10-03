//! Real Core ownership through pause continuation, prompt normalization, rollout
//! reload and copied fork. Exact request content is the acceptance assertion.
use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_pause_response_ownership_survives_save_resume_and_fork() -> Result<()> {
    skip_if_no_network!(Ok(()));
    for text_only in [true, false] {
        let server = MockServer::start().await;
        let early = if text_only {
            format!(
                "{}{}{}",
                message_start("early"),
                plain_text_block(0, "same"),
                message_delta("pause_turn")
            )
        } else {
            format!(
                "{}{}{}{}",
                message_start("early"),
                server_tool_use_block(0, "srvu_identity", "identity"),
                block_stop(0),
                message_delta("pause_turn")
            )
        };
        let late = if text_only {
            format!(
                "{}{}{}{}{}{}{}{}",
                message_start("late"),
                server_tool_use_block(0, "srvu_identity", "identity"),
                block_stop(0),
                search_result_block(1, "srvu_identity", "ENC_IDENTITY"),
                block_stop(1),
                cited_text_block(2, "same"),
                block_stop(2),
                message_delta("end_turn")
            )
        } else {
            format!(
                "{}{}{}{}{}{}",
                message_start("late"),
                search_result_block(0, "srvu_identity", "ENC_IDENTITY"),
                block_stop(0),
                cited_text_block(1, "same"),
                block_stop(1),
                message_delta("end_turn")
            )
        };
        let follow = format!(
            "{}{}{}",
            message_start("done"),
            plain_text_block(0, "answered"),
            message_delta("end_turn")
        );
        let responder = mount_anthropic_sequence(
            &server,
            vec![early, late, follow.clone(), follow.clone(), follow],
        )
        .await;
        let provider = anthropic_provider(&server.uri());
        let initial = test_codex()
            .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
                /*auth*/ None,
            ))
            .with_history_mode(codex_protocol::protocol::ThreadHistoryMode::Legacy)
            .with_config({
                let provider = provider.clone();
                move |config| config.model_provider = provider
            })
            .build_with_auto_env(&server)
            .await?;
        initial.submit_text_turn("search").await?;
        initial.submit_text_turn("follow up").await?;
        let path = initial.codex.rollout_path().context("identity rollout")?;
        initial.codex.shutdown_and_wait().await?;
        let before = responder.captured();
        assert_eq!(before.len(), 3, "one pause continuation plus next turn");
        let mut content = if text_only {
            vec![json!({"type":"text","text":"same"})]
        } else {
            Vec::new()
        };
        content.extend(search_replay_blocks(
            "srvu_identity",
            "identity",
            "ENC_IDENTITY",
        ));
        content.push(cited_replay_block("same"));
        assert_eq!(
            before[2]["messages"],
            expected_follow_up_messages(&before[0]["messages"], content, "follow up"),
            "normalization must preserve ownership; text_only={text_only}"
        );

        // Fork copies the persisted checkpoint before the source is resumed.
        let fork = initial
            .thread_manager
            .fork_legacy_thread(
                codex_core::ForkSnapshot::Interrupted,
                codex_core::StartThreadOptions::new(initial.config.clone()),
                path.clone(),
            )
            .await?
            .thread;
        fork.start_or_steer_turn(codex_core::TurnInputRequest::user_input(vec![
            codex_protocol::user_input::UserInput::Text {
                text: "after fork".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
        let complete = core_test_support::wait_for_event(&fork, |event| {
            matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
        })
        .await;
        let codex_protocol::protocol::EventMsg::TurnComplete(complete) = complete else {
            anyhow::bail!("fork did not complete");
        };
        assert_eq!(complete.error, None);
        fork.shutdown_and_wait().await?;

        let cwd = initial.config.cwd.clone();
        let resumed = test_codex()
            .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
                /*auth*/ None,
            ))
            .with_config(move |config| {
                config.model_provider = provider;
                config.cwd = cwd;
            })
            .resume(&server, initial.home.clone(), path)
            .await?;
        resumed.submit_text_turn("after resume").await?;
        resumed.codex.shutdown_and_wait().await?;
        let bodies = responder.captured();
        assert_eq!(bodies.len(), 5);
        let original = before[2]["messages"]
            .as_array()
            .context("original messages")?;
        for body in &bodies[3..] {
            let messages = body["messages"].as_array().context("reloaded messages")?;
            assert_eq!(
                &messages[..original.len()],
                original.as_slice(),
                "saved response and segment provenance preserves the complete prefix; text_only={text_only}"
            );
        }
    }
    Ok(())
}
