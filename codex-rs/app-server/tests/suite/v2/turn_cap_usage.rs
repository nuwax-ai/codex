//! v2 RPC coverage for output-cap terminals: the failed sampling's usage
//! must reach the shared accounting (thread/tokenUsage/updated) exactly like
//! a completed response's would, while the turn itself fails with no
//! synthesized completion.

use anyhow::Context;
use anyhow::Result;
use app_test_support::MockResponsesConfig;
use app_test_support::TestAppServer;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ThreadStartParams;
use codex_app_server_protocol::ThreadStartResponse;
use codex_app_server_protocol::TurnStartParams;
use codex_app_server_protocol::TurnStartResponse;
use codex_app_server_protocol::UserInput as V2UserInput;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn cap_terminal_sse_response() -> String {
    let frames = [json!({"type": "response.created", "response": {"id": "resp_cap_v2"}}),
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
        })];
    frames
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect()
}

#[tokio::test]
async fn capped_sampling_reports_terminal_usage_over_v2() -> Result<()> {
    let server = app_test_support::create_mock_responses_server_sequence_unchecked(vec![
        cap_terminal_sse_response(),
    ])
    .await;

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

    let _: TurnStartResponse = mcp
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
    let error = completed["turn"]["error"]
        .as_object()
        .context("the capped turn completes with an error")?;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("Output token limit reached")),
        "the terminal names the output cap: {error:?}"
    );

    // The capped sampling's usage reaches the shared accounting: the last
    // snapshot is the terminal frame's counters, surfaced as a
    // thread/tokenUsage/updated notification.
    let usage = token_usage_notifications
        .last()
        .context("at least one token usage notification for the capped sampling")?;
    assert_eq!(
        usage["tokenUsage"]["last"]["totalTokens"],
        json!(14),
        "the terminal frame's reported counters survive to the v2 surface: {usage:?}"
    );
    assert_eq!(
        usage["tokenUsage"]["total"]["totalTokens"],
        json!(14),
        "the thread total includes the capped sampling"
    );
    assert_eq!(raw_response_completions, 0, "no completed raw response");
    Ok(())
}
