//! v2 RPC coverage for output-cap terminals: the failed sampling's usage
//! contributes to shared spending while preserving the last installed context
//! baseline. The turn fails without a synthesized completion.

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ThreadResumeParams;
use codex_app_server_protocol::ThreadResumeResponse;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::ThreadTokenUsageUpdatedNotification;
use codex_app_server_protocol::TokenUsageBreakdown;
use codex_app_server_protocol::TurnCompletedNotification;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::TurnStatus;
use codex_app_server_protocol::UserInput as V2UserInput;
use core_test_support::responses;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use test_case::test_case;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn cap_terminal_sse_response() -> String {
    let frames = [
        json!({"type": "response.created", "response": {"id": "resp_cap_v2"}}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_cap_v2", "role": "assistant", "content": []}}),
        json!({"type": "response.output_text.delta", "item_id": "msg_cap_v2", "delta": "cut short"}),
        json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_cap_v2",
                "incomplete_details": {"reason": "max_output_tokens"},
                "usage": {
                    "input_tokens": 9,
                    "output_tokens": 5,
                    "total_tokens": 14
                }
            }
        }),
    ];
    frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect()
}

#[test_case(0; "without_installed_sampling")]
#[test_case(33; "after_completed_sampling")]
#[tokio::test]
async fn capped_sampling_reports_terminal_usage_over_v2(previous_tokens: i64) -> Result<()> {
    let mut sampling_responses = Vec::new();
    if previous_tokens > 0 {
        sampling_responses.push(responses::sse(vec![
            responses::ev_response_created("resp_complete_v2"),
            responses::ev_assistant_message("msg_complete_v2", "complete answer"),
            responses::ev_completed_with_tokens("resp_complete_v2", previous_tokens),
        ]));
    }
    sampling_responses.push(cap_terminal_sse_response());
    let server =
        app_test_support::create_mock_responses_server_sequence_unchecked(sampling_responses).await;

    let codex_home = TempDir::new()?;
    MockResponsesConfig::new(&server.uri()).write(codex_home.path())?;

    let mut mcp = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;

    let ThreadStartResponse { thread, .. } = mcp
        .start_thread(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;

    if previous_tokens > 0 {
        let _: TurnStartResponse = mcp
            .request(|request_id| ClientRequest::TurnStart {
                request_id,
                params: TurnStartParams {
                    thread_id: thread.id.clone(),
                    input: vec![V2UserInput::Text {
                        text: "install a successful context baseline".to_string(),
                        text_elements: Vec::new(),
                    }],
                    ..Default::default()
                },
            })
            .await?;
        let completed: TurnCompletedNotification = tokio::time::timeout(
            DEFAULT_READ_TIMEOUT,
            mcp.read_notification("turn/completed"),
        )
        .await??;
        assert_eq!(
            (completed.turn.status, completed.turn.error),
            (TurnStatus::Completed, None)
        );
    }

    let capped_turn: TurnStartResponse = mcp
        .request(|request_id| ClientRequest::TurnStart {
            request_id,
            params: TurnStartParams {
                thread_id: thread.id.clone(),
                input: vec![V2UserInput::Text {
                    text: "hit the cap".to_string(),
                    text_elements: Vec::new(),
                }],
                ..Default::default()
            },
        })
        .await?;

    let mut token_usage_notifications = Vec::new();
    let mut turn_completed = None;
    let mut raw_response_completions = 0usize;
    tokio::time::timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let notification = mcp
                .read_stream_until_matching_notification(
                    "token usage or turn completion",
                    |notification| {
                        matches!(
                            notification.method.as_str(),
                            "thread/tokenUsage/updated"
                                | "turn/completed"
                                | "rawResponseItem/completed"
                                | "item/completed"
                        )
                    },
                )
                .await?;
            // Reads for the first turn's completion can leave its other
            // notifications buffered. Only this failed turn belongs to the
            // no-completed-output and terminal-usage assertions below.
            let params = notification
                .params
                .as_ref()
                .context("turn-scoped notification params")?;
            let notified_turn_id = if notification.method == "turn/completed" {
                params["turn"]["id"].as_str()
            } else {
                params["turnId"].as_str()
            }
            .context("turn-scoped notification turn id")?;
            if notified_turn_id != capped_turn.turn.id.as_str() {
                continue;
            }
            match notification.method.as_str() {
                "thread/tokenUsage/updated" => {
                    let params = notification
                        .params
                        .clone()
                        .context("token usage notification params")?;
                    token_usage_notifications.push(params);
                }
                "rawResponseItem/completed" => {
                    raw_response_completions += 1;
                }
                "item/completed" => {
                    // The streamed partial must not become an ordinary
                    // completed assistant item.
                    let item_type = notification
                        .params
                        .as_ref()
                        .context("item notification params")?["item"]["type"]
                        .as_str();
                    anyhow::ensure!(
                        item_type != Some("agentMessage"),
                        "a capped turn must not complete the partial agent message, got {item_type:?}"
                    );
                }
                "turn/completed" => {
                    turn_completed = Some(
                        notification
                            .params
                            .clone()
                            .context("turn completed params")?,
                    );
                    return Ok::<(), anyhow::Error>(());
                }
                _ => {}
            }
        }
    })
    .await
    .context("timed out waiting for the capped turn to complete")??;

    let completed = turn_completed.context("turn/completed arrived")?;
    assert_eq!(completed["turn"]["status"], json!("failed"));
    let error = completed["turn"]["error"]
        .as_object()
        .context("the capped turn completes with an error")?;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("Output token limit reached")),
        "the terminal names the output cap: {error:?}"
    );

    // The original fork test expected `last` to equal the cap counters. After
    // integrating upstream's SpendingOnly accounting, `last` is the installed
    // context baseline: discarded partial output must not replace it. Real spend
    // still enters `total`, including when no sampling has completed yet.
    let usage: ThreadTokenUsageUpdatedNotification = serde_json::from_value(
        token_usage_notifications
            .last()
            .context("at least one token usage notification for the capped sampling")?
            .clone(),
    )?;
    assert_eq!(
        (usage.thread_id.as_str(), usage.turn_id.as_str()),
        (
            thread.id.as_str(),
            completed["turn"]["id"].as_str().context("capped turn id")?,
        )
    );
    assert_eq!(
        (&usage.token_usage.last, &usage.token_usage.total),
        (
            &TokenUsageBreakdown {
                input_tokens: previous_tokens,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                total_tokens: previous_tokens,
            },
            &TokenUsageBreakdown {
                input_tokens: previous_tokens + 9,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 5,
                reasoning_output_tokens: 0,
                total_tokens: previous_tokens + 14,
            }
        ),
        "the capped sampling changes spending without installing discarded output"
    );
    assert_eq!(raw_response_completions, 0, "no completed raw response");

    // A transient notification override would corrupt this baseline on cold
    // reconstruction: TokenCount events are also persisted into the rollout.
    assert!(mcp.shutdown_gracefully().await?.success());
    let mut resumed = TestAppServer::builder()
        .with_codex_home(codex_home.path())
        .build_initialized()
        .await?;
    let resume_id = resumed
        .send_thread_resume_request(ThreadResumeParams {
            thread_id: thread.id.clone(),
            ..Default::default()
        })
        .await?;
    let response: ThreadResumeResponse =
        tokio::time::timeout(DEFAULT_READ_TIMEOUT, resumed.read_response(resume_id)).await??;
    assert_eq!(response.thread.id, thread.id);
    let replayed: ThreadTokenUsageUpdatedNotification = tokio::time::timeout(
        DEFAULT_READ_TIMEOUT,
        resumed.read_notification("thread/tokenUsage/updated"),
    )
    .await??;
    assert_eq!(
        replayed, usage,
        "cold attach restores the same baseline and spending"
    );
    assert!(resumed.shutdown_gracefully().await?.success());
    Ok(())
}
