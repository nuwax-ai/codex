//! Offline verification of the hard pre-dispatch HTTP attempt budget
//! (`CODEX_RIG_MAX_HTTP_ATTEMPTS`): the loopback server's connection count is
//! the authority that a refused attempt NEVER reached the wire — log capture
//! line counts alone are not accepted as budget evidence.
//!
//! The REAL `codex-exec` binary drives the marker tool round trip against a
//! scripted Chat SSE loopback. A budget that exactly covers the needed
//! attempts must not interfere; a budget one short must fail the turn through
//! the product's own error path with the refusal visible in the retained
//! artifacts, exactly one dispatched attempt on the ledger, and NO second
//! connection server-side (the listener stays open the whole time, so a
//! dispatched-but-refused connection could not hide behind ECONNREFUSED).
//!
//! ```text
//! cargo build -p codex-exec --bin codex-exec
//! cargo nextest run -p codex-live-tests --test nuwax_env_budget_offline --no-capture
//! ```

mod nuwax_loopback_common;

use anyhow::Result;
use codex_live_tests::LiveConfig;
use codex_live_tests::codex_exec_binary;
use codex_live_tests::run_nuwax_env_marker_turn;
use nuwax_loopback_common::API_KEY;
use nuwax_loopback_common::MODEL;
use nuwax_loopback_common::OUTPUT_CAP;
use nuwax_loopback_common::advertised_shell_tool;
use nuwax_loopback_common::assert_request_shape;
use nuwax_loopback_common::final_answer_frames;
use nuwax_loopback_common::marker_from_prompt;
use nuwax_loopback_common::newest_artifacts_dir;
use nuwax_loopback_common::read_request;
use nuwax_loopback_common::respond_sse;
use nuwax_loopback_common::tool_call_frames;
use pretty_assertions::assert_eq;
use std::time::Duration;
use tokio::net::TcpListener;

fn offline_config(addr: std::net::SocketAddr, vendor: &str) -> LiveConfig {
    LiveConfig {
        vendor: vendor.to_string(),
        api_key: API_KEY.to_string(),
        base_url: format!("http://{addr}/v1"),
        anthropic_base_url: None,
        responses_base_url: None,
        model: MODEL.to_string(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn budget_exactly_covering_the_round_trip_completes_normally() -> Result<()> {
    codex_exec_binary()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let mut marker = String::new();
        for index in 0..2 {
            let (mut socket, _) = listener.accept().await?;
            let (headers, body) = read_request(&mut socket).await?;
            assert_request_shape(&headers, &body)?;
            if index == 0 {
                let extracted = marker_from_prompt(&body)?;
                let tool = advertised_shell_tool(&body)?;
                respond_sse(&mut socket, &tool_call_frames(tool, &extracted)).await?;
                marker = extracted;
            } else {
                respond_sse(&mut socket, &final_answer_frames(&marker)).await?;
            }
        }
        anyhow::Ok(marker)
    });

    let cfg = offline_config(addr, "nuwax-budget-ok");
    run_nuwax_env_marker_turn(
        /*protocol*/ "chat-budget-ok",
        &cfg,
        &cfg.base_url,
        "chat",
        Some(OUTPUT_CAP),
        "",
        Some("Dispatching chat stream via rig"),
        /*http_attempt_budget*/ Some(2),
    )
    .await?;

    let marker = tokio::time::timeout(Duration::from_secs(30), server).await???;
    assert!(!marker.is_empty(), "marker extracted server-side");
    let artifacts = newest_artifacts_dir("nuwax-budget-ok")?;
    let ledger = std::fs::read_to_string(artifacts.join("requests.jsonl"))?;
    assert_eq!(
        ledger
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        2,
        "both dispatched attempts are on the capture ledger"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn budget_one_short_refuses_the_continuation_before_any_dispatch() -> Result<()> {
    codex_exec_binary()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    // Serves ONLY the first sampling, then keeps the listener open and watches
    // for a forbidden second connection: the continuation after the tool
    // result must be refused client-side before any dispatch, so the watch
    // must time out with the connection count still at one.
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let (headers, body) = read_request(&mut socket).await?;
        assert_request_shape(&headers, &body)?;
        let marker = marker_from_prompt(&body)?;
        let tool = advertised_shell_tool(&body)?;
        respond_sse(&mut socket, &tool_call_frames(tool, &marker)).await?;
        let second = tokio::time::timeout(Duration::from_secs(15), listener.accept()).await;
        anyhow::Ok((marker, second.is_err()))
    });

    let cfg = offline_config(addr, "nuwax-budget-short");
    let outcome = run_nuwax_env_marker_turn(
        /*protocol*/ "chat-budget-short",
        &cfg,
        &cfg.base_url,
        "chat",
        Some(OUTPUT_CAP),
        "",
        Some("Dispatching chat stream via rig"),
        /*http_attempt_budget*/ Some(1),
    )
    .await;
    assert!(
        outcome.is_err(),
        "a one-short budget cannot complete the turn"
    );

    let (marker, no_second_connection) =
        tokio::time::timeout(Duration::from_secs(30), server).await???;
    assert!(!marker.is_empty(), "first exchange served before refusal");
    assert!(
        no_second_connection,
        "the refused continuation never reached the server"
    );

    let artifacts = newest_artifacts_dir("nuwax-budget-short")?;
    let events = std::fs::read_to_string(artifacts.join("events.jsonl")).unwrap_or_default();
    assert!(
        events.contains("HTTP attempt budget of 1 exhausted"),
        "the refusal must surface as the turn's terminal error: {events}"
    );
    assert!(
        !events.contains("turn.completed"),
        "a budget refusal must not masquerade as a completed turn: {events}"
    );
    let ledger = std::fs::read_to_string(artifacts.join("requests.jsonl"))?;
    assert_eq!(
        ledger
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        1,
        "the refused attempt never entered the ledger"
    );
    Ok(())
}
