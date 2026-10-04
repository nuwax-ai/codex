//! Core-level closed-loop coverage for the fork's Anthropic bridge wire:
//! real Codex turns (client tool execution, hosted search pairing, save and
//! resume) against a mock Anthropic gateway, with deep comparison of the
//! consecutive request bodies this fork actually sends.
//!
//! Complements the bridge wire tests (transport-level) by running the full
//! agent loop: history items are produced by real events and persisted
//! through the rollout, not hand-rebuilt requests.
//!
//! Feature gate: `rust-rig` is NOT a default feature of `codex-core`, so a
//! single-package run must enable it explicitly or these tests silently
//! match zero:
//! `just test -p codex-core --features rust-rig -E 'test(rig_anthropic)'`
//! (a joint `-p codex-core -p codex-app-server -p codex-exec` invocation
//! also works through feature unification). Verify the selected count is
//! non-zero before treating a green run as coverage.

#![cfg(feature = "rust-rig")]

mod support;
use support::*;

use anyhow::Context;
use anyhow::Result;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_mixed_turn_core_loop_preserves_request_prefix() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse_turn1 = format!(
        "{}{}{}{}{}{}",
        message_start("m1"),
        server_tool_use_block(0, "srvu_core", "core-mixed"),
        block_stop(0),
        tool_use_block(1, "toolu_core"),
        block_stop(1),
        message_delta("tool_use"),
    );
    let sse_turn1_late = format!(
        "{}{}{}{}",
        message_start("m2"),
        search_result_block(0, "srvu_core", "ENC_CORE_LATE"),
        block_stop(0),
        message_delta("end_turn"),
    );
    let sse_final = format!(
        "{}{}{}",
        message_start("m3"),
        plain_text_block(0, "all done"),
        message_delta("end_turn"),
    );
    let responder =
        mount_anthropic_sequence(&server, vec![sse_turn1, sse_turn1_late, sse_final]).await;
    let provider = anthropic_provider(&server.uri());
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("start the mixed turn").await?;
    test.submit_text_turn("continue").await?;

    let bodies = responder.captured();
    assert_eq!(bodies.len(), 3, "three model requests across the loop");
    // bodies[0] is the turn's initial request; the tool output follow-up
    // (still carrying the PENDING search) is bodies[1], and the completed
    // history first reaches the model on the NEXT turn, bodies[2].
    let pending_request = &bodies[1];
    let next_request = &bodies[2];

    // While the search was pending, the request carried the client tool
    // call, its output, and the pending server call.
    let pending_messages = pending_request["messages"].as_array().unwrap().clone();
    let pending_encoded = serde_json::to_string(&pending_messages).unwrap();
    assert!(pending_encoded.contains("toolu_core"), "{pending_encoded}");
    assert!(pending_encoded.contains("srvu_core"), "{pending_encoded}");

    // The next turn's request carries the completed history: EVERYTHING the
    // pending request sent stays byte-identical (the pending assistant keeps
    // its use block exactly where it was), and the additions are the late
    // result at its own new response position plus the new user message.
    let next_messages = next_request["messages"].as_array().unwrap().clone();
    assert_eq!(
        next_messages.len(),
        pending_messages.len() + 2,
        "a new assistant position for the result, plus the new user message"
    );
    assert_eq!(
        &next_messages[..pending_messages.len()],
        &pending_messages[..],
        "the entire pending request stays byte-identical: {} vs {}",
        serde_json::to_string(&next_messages).unwrap(),
        serde_json::to_string(&pending_messages).unwrap(),
    );
    let result_assistant = &next_messages[pending_messages.len()];
    assert_eq!(result_assistant["role"], json!("assistant"));
    assert_eq!(
        result_assistant["content"],
        json!([{"type":"web_search_tool_result","tool_use_id":"srvu_core","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC_CORE_LATE"}]}]),
        "the late result replays WITHOUT re-sending the call: {}",
        serde_json::to_string(result_assistant).unwrap()
    );
    let trailing = next_messages.last().unwrap();
    assert_eq!(trailing["role"], json!("user"));
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

/// A completed search turn with a cited answer: the saved assistant Message
/// and the envelope's cited text describe the SAME streamed answer, so the
/// next request carries it exactly once with its citations, next to a second
/// uncited text block that stays untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cited_answer_appears_once_with_citations() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse_search = format!(
        "{}{}{}{}{}{}{}{}{}",
        message_start("m1"),
        server_tool_use_block(0, "srvu_cited", "core-cited"),
        block_stop(0),
        search_result_block(1, "srvu_cited", "ENC_CORE_CIT"),
        block_stop(1),
        cited_text_block(2, "the cited core answer"),
        block_stop(2),
        plain_text_block(3, "plain tail"),
        message_delta("end_turn"),
    );
    let sse_final = format!(
        "{}{}{}",
        message_start("m2"),
        plain_text_block(0, "bye"),
        message_delta("end_turn"),
    );
    let responder = mount_anthropic_sequence(&server, vec![sse_search, sse_final]).await;
    let provider = anthropic_provider(&server.uri());
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("search and answer").await?;
    test.submit_text_turn("again").await?;

    let bodies = responder.captured();
    assert_eq!(bodies.len(), 2);
    let mut content = search_replay_blocks("srvu_cited", "core-cited", "ENC_CORE_CIT");
    content.push(cited_replay_block("the cited core answer"));
    content.push(json!({"type":"text","text":"plain tail"}));
    assert_eq!(
        bodies[1]["messages"],
        expected_follow_up_messages(&bodies[0]["messages"], content, "again"),
        "the entire next request preserves the prefix, search pair, citation and uncited tail"
    );
    let encoded = serde_json::to_string(&bodies[1]["messages"])?;
    assert_eq!(encoded.matches("the cited core answer").count(), 1);
    assert_eq!(encoded.matches("plain tail").count(), 1);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

/// N2: an uncited introduction before the hosted search must remain before
/// the search pair when the later cited answer and uncited tail are replayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cited_replay_preserves_intro_search_answer_tail_order() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let first = format!(
        "{}{}{}{}{}{}{}{}{}{}",
        message_start("m1"),
        plain_text_block(0, "plain intro"),
        server_tool_use_block(1, "srvu_order", "core-order"),
        block_stop(1),
        search_result_block(2, "srvu_order", "ENC_CORE_ORDER"),
        block_stop(2),
        cited_text_block(3, "ordered cited answer"),
        block_stop(3),
        plain_text_block(4, "plain tail"),
        message_delta("end_turn"),
    );
    let follow_up = format!(
        "{}{}{}",
        message_start("m2"),
        plain_text_block(0, "done"),
        message_delta("end_turn"),
    );
    let responder = mount_anthropic_sequence(&server, vec![first, follow_up]).await;
    let provider = anthropic_provider(&server.uri());
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("search and answer").await?;
    test.submit_text_turn("again").await?;

    let bodies = responder.captured();
    assert_eq!(bodies.len(), 2);
    let mut content = vec![json!({"type":"text","text":"plain intro"})];
    content.extend(search_replay_blocks(
        "srvu_order",
        "core-order",
        "ENC_CORE_ORDER",
    ));
    content.push(cited_replay_block("ordered cited answer"));
    content.push(json!({"type":"text","text":"plain tail"}));
    assert_eq!(
        bodies[1]["messages"],
        expected_follow_up_messages(&bodies[0]["messages"], content, "again"),
        "replay preserves the exact original introduction, search, cited answer and tail order"
    );
    let encoded = serde_json::to_string(&bodies[1]["messages"])?;
    assert_eq!(encoded.matches("ordered cited answer").count(), 1);
    assert_eq!(encoded.matches("plain intro").count(), 1);
    assert_eq!(encoded.matches("plain tail").count(), 1);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

/// N2: repeated text in an earlier uncited block must not steal the citation
/// from the later block that actually carried it in the provider response.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_cited_replay_keeps_repeated_text_citation_owner() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let first = format!(
        "{}{}{}{}{}{}{}{}{}{}",
        message_start("m1"),
        plain_text_block(0, "Uncited shared phrase intro. "),
        server_tool_use_block(1, "srvu_owner", "core-owner"),
        block_stop(1),
        search_result_block(2, "srvu_owner", "ENC_CORE_OWNER"),
        block_stop(2),
        cited_text_block(3, "shared phrase"),
        block_stop(3),
        plain_text_block(4, " plain tail"),
        message_delta("end_turn"),
    );
    let follow_up = format!(
        "{}{}{}",
        message_start("m2"),
        plain_text_block(0, "done"),
        message_delta("end_turn"),
    );
    let responder = mount_anthropic_sequence(&server, vec![first, follow_up]).await;
    let provider = anthropic_provider(&server.uri());
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_text_turn("search and answer").await?;
    test.submit_text_turn("again").await?;

    let bodies = responder.captured();
    assert_eq!(bodies.len(), 2);
    let mut content = vec![json!({"type":"text","text":"Uncited shared phrase intro. "})];
    content.extend(search_replay_blocks(
        "srvu_owner",
        "core-owner",
        "ENC_CORE_OWNER",
    ));
    content.push(cited_replay_block("shared phrase"));
    content.push(json!({"type":"text","text":" plain tail"}));
    assert_eq!(
        bodies[1]["messages"],
        expected_follow_up_messages(&bodies[0]["messages"], content, "again"),
        "the earlier occurrence stays wholly uncited; only the later streamed block owns the citation"
    );
    let encoded = serde_json::to_string(&bodies[1]["messages"])?;
    assert_eq!(encoded.matches("shared phrase").count(), 2);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

/// Save a completed mixed turn, close the session, and resume it: the first
/// request after resume must replay the identical message projection the
/// pre-close turns produced (save/restore does not rewrite history).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_resume_replays_the_same_projection() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let sse_turn1 = format!(
        "{}{}{}{}{}{}",
        message_start("m1"),
        server_tool_use_block(0, "srvu_resume", "core-resume"),
        block_stop(0),
        search_result_block(1, "srvu_resume", "ENC_CORE_RESUME"),
        block_stop(1),
        message_delta("end_turn"),
    );
    let sse_turn2 = format!(
        "{}{}{}",
        message_start("m2"),
        plain_text_block(0, "answered"),
        message_delta("end_turn"),
    );
    let sse_after_resume = sse_turn2.replace("m2", "m3");
    let responder =
        mount_anthropic_sequence(&server, vec![sse_turn1, sse_turn2, sse_after_resume]).await;

    let provider = anthropic_provider(&server.uri());
    let initial = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config({
            let provider = provider.clone();
            move |config| {
                config.model_provider = provider;
            }
        })
        .build_with_auto_env(&server)
        .await?;
    initial.submit_text_turn("search now").await?;
    initial.submit_text_turn("wrap up").await?;
    let rollout_path = initial.codex.rollout_path().context("rollout path")?;
    initial.codex.shutdown_and_wait().await?;

    let original_bodies = responder.requests.lock().unwrap().clone();
    assert_eq!(original_bodies.len(), 2);
    let original_messages = original_bodies[1]["messages"].as_array().unwrap().clone();

    let original_cwd = initial.config.cwd.clone();
    let resumed = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
            config.cwd = original_cwd;
        })
        .resume(&server, initial.home.clone(), rollout_path.clone())
        .await?;
    resumed.submit_text_turn("after resume").await?;
    resumed.codex.shutdown_and_wait().await?;

    let bodies = responder.captured();
    assert_eq!(bodies.len(), 3);
    let resumed_messages = bodies[2]["messages"].as_array().unwrap().clone();
    assert!(
        resumed_messages.len() > original_messages.len(),
        "the new user turn extends the history"
    );
    assert_eq!(
        &resumed_messages[..original_messages.len()],
        &original_messages[..],
        "resume replays the byte-identical pre-close projection"
    );
    Ok(())
}

/// C2/N5: interrupting a request before any response headers arrive closes
/// its socket and leaves the immediately following turn usable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_interrupt_before_headers_closes_socket_and_allows_follow_up() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (request_ready_tx, request_ready_rx) = oneshot::channel();
    let (socket_closed_tx, socket_closed_rx) = oneshot::channel();
    let follow_up = format!(
        "{}{}{}",
        message_start("m2"),
        plain_text_block(0, "follow-up completed"),
        message_delta("end_turn"),
    );
    let server_task = tokio::spawn(async move {
        let (mut first_socket, _) = listener.accept().await.context("accept stalled request")?;
        let first_request = read_anthropic_request(&mut first_socket).await?;
        request_ready_tx
            .send(())
            .map_err(|_| anyhow::anyhow!("request-ready receiver closed"))?;
        // Send no headers or body: an EOF/reset here proves that Interrupt
        // cancels the HTTP request while reqwest is still waiting for headers.
        let mut probe = [0; 1];
        let closed =
            match tokio::time::timeout(Duration::from_secs(3), first_socket.read(&mut probe)).await
            {
                Ok(Ok(0)) => Ok(()),
                Ok(Err(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::BrokenPipe
                    ) =>
                {
                    Ok(())
                }
                Ok(Ok(read)) => Err(format!("stalled socket received {read} unexpected bytes")),
                Ok(Err(error)) => Err(format!("observe interrupted socket closure: {error}")),
                Err(error) => Err(format!(
                    "interrupted request socket did not close within three seconds: {error}"
                )),
            };
        drop(first_socket);
        socket_closed_tx
            .send(closed.clone())
            .map_err(|_| anyhow::anyhow!("socket-closed receiver closed"))?;
        closed.map_err(anyhow::Error::msg)?;

        let (mut second_socket, _) =
            tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .context("follow-up request did not arrive")?
                .context("accept follow-up request")?;
        let second_request = read_anthropic_request(&mut second_socket).await?;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{follow_up}",
            follow_up.len()
        );
        second_socket
            .write_all(response.as_bytes())
            .await
            .context("write complete follow-up SSE")?;
        drop(second_socket);
        anyhow::ensure!(
            tokio::time::timeout(Duration::from_millis(750), listener.accept())
                .await
                .is_err(),
            "interrupt or follow-up produced an extra model connection"
        );
        Ok::<_, anyhow::Error>(vec![first_request, second_request])
    });
    let server = MockServer::start().await;
    let provider = anthropic_provider(&format!("http://{address}"));
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    let codex = std::sync::Arc::clone(&test.codex);
    codex
        .start_or_steer_turn(codex_core::TurnInputRequest::user_input(vec![
            codex_protocol::user_input::UserInput::Text {
                text: "stall".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    tokio::time::timeout(Duration::from_secs(10), request_ready_rx)
        .await
        .context("complete stalled request did not reach the server")?
        .context("stalled-request server stopped before readiness")?;
    codex
        .submit(codex_protocol::protocol::Op::Interrupt)
        .await?;
    core_test_support::wait_for_event(&codex, |event| {
        matches!(event, codex_protocol::protocol::EventMsg::TurnAborted(_))
    })
    .await;
    // Submit immediately after TurnAborted so the same Core client must be
    // usable without a manual reset or delayed retry.
    codex
        .start_or_steer_turn(codex_core::TurnInputRequest::user_input(vec![
            codex_protocol::user_input::UserInput::Text {
                text: "follow up immediately".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    tokio::time::timeout(Duration::from_secs(4), socket_closed_rx)
        .await
        .context("first HTTP socket was not released after Interrupt")?
        .context("server failed while observing first socket closure")?
        .map_err(anyhow::Error::msg)?;
    let mut assistant_messages = Vec::new();
    let completed = core_test_support::wait_for_event(&codex, |event| {
        if let codex_protocol::protocol::EventMsg::AgentMessage(message) = event {
            assistant_messages.push(message.message.clone());
        }
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;
    let codex_protocol::protocol::EventMsg::TurnComplete(completed) = completed else {
        anyhow::bail!("expected follow-up TurnComplete");
    };
    assert_eq!(completed.error, None, "immediate follow-up must succeed");
    assert_eq!(
        completed.last_agent_message.as_deref(),
        Some("follow-up completed")
    );
    assert_eq!(assistant_messages, vec!["follow-up completed"]);
    let requests = server_task.await.context("join raw Anthropic server")??;
    assert_eq!(
        requests.len(),
        2,
        "exactly the interrupted request and successful immediate follow-up"
    );
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

/// C2/N5: interrupting a long-running client tool aborts the turn, records
/// the call with an aborted output, and does not re-execute the tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_interrupt_during_tool_runs_it_once() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = MockServer::start().await;
    let args = r#"{"cmd":"sleep 60","yield_time_ms":60000}"#;
    let tool_call = format!(
        "event: content_block_start\ndata: {{\"type\":\"content_block_start\",\"index\":1,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_sleep\",\"name\":\"exec_command\",\"input\":{{}}}}}}\n\nevent: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":1,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":{args:?}}}}}\n\n"
    );
    let first = format!(
        "{}{}{}{}",
        message_start("m1"),
        tool_call,
        block_stop(1),
        message_delta("tool_use")
    );
    let follow_up = format!(
        "{}{}{}",
        message_start("m2"),
        plain_text_block(0, "resumed"),
        message_delta("end_turn"),
    );
    let responder = mount_anthropic_sequence(&server, vec![first, follow_up]).await;
    let provider = anthropic_provider(&server.uri());
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build_with_auto_env(&server)
        .await?;
    let codex = std::sync::Arc::clone(&test.codex);
    codex
        .start_or_steer_turn(codex_core::TurnInputRequest::user_input(vec![
            codex_protocol::user_input::UserInput::Text {
                text: "run the tool".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    let mut begins = 0usize;
    let begin = core_test_support::wait_for_event(&codex, |event| {
        if matches!(
            event,
            codex_protocol::protocol::EventMsg::ExecCommandBegin(_)
        ) {
            begins += 1;
        }
        matches!(
            event,
            codex_protocol::protocol::EventMsg::ExecCommandBegin(_)
                | codex_protocol::protocol::EventMsg::ExecApprovalRequest(_)
                | codex_protocol::protocol::EventMsg::Error(_)
                | codex_protocol::protocol::EventMsg::TurnComplete(_)
                | codex_protocol::protocol::EventMsg::TurnAborted(_)
        )
    })
    .await;
    let captured_requests = responder.captured();
    let tool_outputs: Vec<_> = captured_requests
        .iter()
        .flat_map(|request| request["messages"].as_array().into_iter().flatten())
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result")
        .cloned()
        .collect();
    anyhow::ensure!(
        matches!(
            begin,
            codex_protocol::protocol::EventMsg::ExecCommandBegin(_)
        ),
        "tool execution did not begin: {begin:?}; tool outputs: {tool_outputs:?}"
    );
    codex
        .submit(codex_protocol::protocol::Op::Interrupt)
        .await?;
    core_test_support::wait_for_event(&codex, |event| {
        if matches!(
            event,
            codex_protocol::protocol::EventMsg::ExecCommandBegin(_)
        ) {
            begins += 1;
        }
        matches!(event, codex_protocol::protocol::EventMsg::TurnAborted(_))
    })
    .await;

    codex
        .start_or_steer_turn(codex_core::TurnInputRequest::user_input(vec![
            codex_protocol::user_input::UserInput::Text {
                text: "follow up".into(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    let completed = core_test_support::wait_for_event(&codex, |event| {
        if matches!(
            event,
            codex_protocol::protocol::EventMsg::ExecCommandBegin(_)
        ) {
            begins += 1;
        }
        matches!(event, codex_protocol::protocol::EventMsg::TurnComplete(_))
    })
    .await;
    let codex_protocol::protocol::EventMsg::TurnComplete(completed) = completed else {
        anyhow::bail!("expected tool-abort follow-up TurnComplete");
    };
    assert_eq!(completed.error, None, "tool-abort follow-up must succeed");
    assert_eq!(completed.last_agent_message.as_deref(), Some("resumed"));

    let requests = responder.captured();
    assert_eq!(requests.len(), 2, "no extra model requests after abort");
    let encoded = serde_json::to_string(&requests[1]).unwrap();
    assert!(encoded.contains("toolu_sleep"), "{encoded}");
    assert!(
        encoded.contains("aborted by user"),
        "the aborted output joins the history: {encoded}"
    );
    assert_eq!(begins, 1, "the tool must execute exactly once");
    test.codex.shutdown_and_wait().await?;
    Ok(())
}

#[path = "rig_anthropic_identity_tests.rs"]
mod identity_tests;

#[path = "rig_anthropic_credential_instance_tests.rs"]
mod credential_instance_tests;
