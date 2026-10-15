//! Image-bearing history through the rig bridge wires: what the producing
//! session put on the wire must survive closing that session and resuming
//! from the rollout (the identity suites prove this for hosted/opaque
//! content; images are the uncovered case), and local compaction must keep
//! the thread usable with images in history.
//!
//! Each resume request keeps the image in its typed wire field (Chat
//! `image_url`, Anthropic base64 `source`, Responses `input_image`) with the
//! exact payload the producing session sent, alongside the intact
//! update_plan call/result pair from the same turn — never a Base64 text
//! copy. Feature gate: run with `--features rust-rig -E 'test(rig_image)'`
//! and verify the selected count is non-zero.

#![cfg(feature = "rust-rig")]

use anyhow::Context;
use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use image::ImageBuffer;
use image::Rgba;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::time::Duration;

fn rig_image_provider(base_url: &str, wire_api: WireApi) -> ModelProviderInfo {
    let protocol = match wire_api {
        WireApi::Chat => "chat",
        WireApi::Responses => "responses",
        WireApi::Anthropic => "anthropic",
    };
    ModelProviderInfo {
        capabilities: None,
        name: format!("rig-image-{protocol}"),
        base_url: Some(format!("{base_url}/v1")),
        model_catalog_url: None,
        env_key: None,
        env_key_instructions: None,
        experimental_bearer_token: None,
        // No explicit bridge: the fork default routes this third-party wire
        // through rig, and provenance is asserted as bridge=rig by the
        // sibling suites for every wire.
        experimental_bridge: None,
        provider_id: Some("rig-image-test".into()),
        auth: None,
        gateway_oauth: None,
        aws: None,
        wire_api,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        stream_idle_timeout_ms: Some(5_000),
        websocket_connect_timeout_ms: None,
        requires_openai_auth: false,
        supports_websockets: false,
        supports_standalone_web_search: false,
        include_internal_metadata: false,
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

fn write_test_png(path: &Path, color: [u8; 4]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let image = ImageBuffer::from_pixel(2, 2, Rgba(color));
    image
        .save(path)
        .with_context(|| format!("save {}", path.display()))?;
    Ok(())
}

fn plan_arguments() -> String {
    json!({"plan": [{"step": "describe the image", "status": "completed"}]}).to_string()
}

fn chat_tool_call_frame(id: &str) -> Value {
    json!({
        "id": "chatcmpl-img", "object": "chat.completion.chunk", "created": 1,
        "model": "image-test", "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0, "id": id, "type": "function",
                "function": {"name": "update_plan", "arguments": plan_arguments()}
            }]},
            "finish_reason": null
        }]
    })
}

fn chat_text_frame(text: &str, finish: Option<&str>) -> Value {
    let mut choice = json!({"index": 0, "delta": {"role": "assistant", "content": text}});
    choice["finish_reason"] = finish.map(Value::from).unwrap_or(Value::Null);
    json!({
        "id": "chatcmpl-img", "object": "chat.completion.chunk", "created": 1,
        "model": "image-test", "choices": [choice],
    })
}

fn chat_tool_finish_frame() -> Value {
    chat_text_frame("", Some("tool_calls"))
}

fn chat_usage_frame() -> Value {
    json!({
        "id": "chatcmpl-img", "object": "chat.completion.chunk", "created": 1,
        "model": "image-test", "choices": [],
        "usage": {"prompt_tokens": 9, "completion_tokens": 4, "total_tokens": 13},
    })
}

fn chat_sse(frames: &[Value]) -> String {
    let mut body = String::new();
    for frame in frames {
        body.push_str(&format!("data: {frame}\n\n"));
    }
    body + "data: [DONE]\n\n"
}

fn anthropic_sse(blocks: Vec<Value>, stop: &str) -> String {
    let mut payload = String::from("event: message_start\ndata: ");
    payload.push_str(
        &json!({"type":"message_start","message":{"id":"msg-img","type":"message","role":"assistant",
            "content":[],"model":"image-test","stop_reason":null,"stop_sequence":null,
            "usage":{"input_tokens":6,"output_tokens":0}}})
        .to_string(),
    );
    payload.push_str("\n\n");
    for (index, block) in blocks.iter().enumerate() {
        payload.push_str(&format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({"type":"content_block_start","index":index,"content_block":block})
        ));
        if block["type"] == "tool_use" {
            payload.push_str(&format!(
                "event: content_block_delta\ndata: {}\n\n",
                json!({"type":"content_block_delta","index":index,
                    "delta":{"type":"input_json_delta","partial_json":plan_arguments()}})
            ));
        } else if block["type"] == "text" {
            payload.push_str(&format!(
                "event: content_block_delta\ndata: {}\n\n",
                json!({"type":"content_block_delta","index":index,
                    "delta":{"type":"text_delta","text":block["text"]}})
            ));
        }
        payload.push_str(&format!(
            "event: content_block_stop\ndata: {}\n\n",
            json!({"type":"content_block_stop","index":index})
        ));
    }
    payload.push_str(&format!(
        "event: message_delta\ndata: {}\n\nevent: message_stop\ndata: {}\n\n",
        json!({"type":"message_delta","delta":{"stop_reason":stop,"stop_sequence":null},
            "usage":{"output_tokens":4}}),
        json!({"type":"message_stop"})
    ));
    payload
}

fn anthropic_tool_use(id: &str) -> Value {
    json!({"type":"tool_use","id":id,"name":"update_plan","input":{}})
}

fn anthropic_text(text: &str) -> Value {
    json!({"type":"text","text":text})
}

/// The inline image payload a request carried, per wire: the Chat data URL,
/// the Anthropic base64 `source.data`, the Responses `input_image` URL.
fn image_payload(wire_api: WireApi, body: &Value) -> Option<String> {
    let scan = |blocks: &Value| -> Option<String> {
        blocks.as_array().and_then(|blocks| {
            blocks.iter().find_map(|block| match wire_api {
                WireApi::Chat => (block["type"] == "image_url")
                    .then(|| block["image_url"]["url"].as_str().map(str::to_string))
                    .flatten(),
                WireApi::Anthropic => (block["type"] == "image"
                    && block["source"]["type"] == "base64")
                    .then(|| block["source"]["data"].as_str().map(str::to_string))
                    .flatten(),
                WireApi::Responses => (block["type"] == "input_image")
                    .then(|| block["image_url"].as_str().map(str::to_string))
                    .flatten(),
            })
        })
    };
    match wire_api {
        WireApi::Chat => body["messages"].as_array().and_then(|messages| {
            messages
                .iter()
                .filter(|message| message["role"] == "user")
                .find_map(|message| scan(&message["content"]))
        }),
        WireApi::Anthropic => body["messages"].as_array().and_then(|messages| {
            messages
                .iter()
                .filter(|message| message["role"] == "user")
                .find_map(|message| scan(&message["content"]))
        }),
        WireApi::Responses => body["input"].as_array().and_then(|items| {
            items
                .iter()
                .filter(|item| item["role"] == "user")
                .find_map(|item| scan(&item["content"]))
        }),
    }
}

fn assert_media_type(wire_api: WireApi, body: &Value) {
    match wire_api {
        WireApi::Anthropic => {
            let media = body["messages"].to_string();
            assert!(
                media.contains("\"media_type\":\"image/png\""),
                "anthropic image source must declare the MIME type: {media}"
            );
        }
        WireApi::Chat | WireApi::Responses => {
            let url = image_payload(wire_api, body).expect("inline image payload");
            assert!(
                url.starts_with("data:image/png;base64,"),
                "chat/responses inline images are PNG data URLs: {url}"
            );
        }
    }
}

/// Captures every POST body for one path, in arrival order.
#[derive(Clone, Default)]
struct RequestLog(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

impl RequestLog {
    fn bodies(&self) -> Vec<Value> {
        self.0.lock().expect("image request log").clone()
    }
}

impl wiremock::Match for RequestLog {
    fn matches(&self, request: &wiremock::Request) -> bool {
        let body = serde_json::from_slice::<Value>(&request.body).expect("request JSON");
        self.0.lock().expect("image request log").push(body);
        true
    }
}

async fn mount_sequence(
    server: &wiremock::MockServer,
    path: &str,
    sse_bodies: Vec<String>,
) -> RequestLog {
    let log = RequestLog::default();
    for body in sse_bodies {
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path(path))
            .and(log.clone())
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_raw(body, "text/event-stream"),
            )
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
    log
}

async fn submit_image_turn(
    codex: &codex_core::CodexThread,
    prompt: &str,
    image_path: &Path,
) -> Result<()> {
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![
            UserInput::LocalImage {
                path: image_path.to_path_buf(),
                detail: None,
            },
            UserInput::Text {
                text: prompt.to_string(),
                text_elements: Vec::new(),
            },
        ]))
        .await?;
    wait_turn_complete(codex).await
}

async fn submit_text_turn_raw(codex: &codex_core::CodexThread, prompt: &str) -> Result<()> {
    codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: prompt.to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_turn_complete(codex).await
}

async fn wait_turn_complete(codex: &codex_core::CodexThread) -> Result<()> {
    let event = tokio::time::timeout(Duration::from_secs(60), async {
        core_test_support::wait_for_event(codex, |event| matches!(event, EventMsg::TurnComplete(_)))
            .await
    })
    .await
    .context("timed out waiting for TurnComplete")?;
    let EventMsg::TurnComplete(complete) = event else {
        unreachable!("matched TurnComplete");
    };
    anyhow::ensure!(
        complete.error.is_none(),
        "turn failed: {:?}",
        complete.error
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn image_lifecycle_wait_rejects_failed_turns() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = wiremock::MockServer::start().await;
    let log = mount_sequence(
        &server,
        "/v1/chat/completions",
        vec![chat_sse(&[
            chat_text_frame("partial answer", Some("length")),
            chat_usage_frame(),
        ])],
    )
    .await;
    let provider = rig_image_provider(&server.uri(), WireApi::Chat);
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| config.model_provider = provider)
        .build_with_auto_env(&server)
        .await?;
    let error = submit_text_turn_raw(&test.codex, "hit the output cap")
        .await
        .expect_err("a failed TurnComplete must not satisfy the lifecycle test");
    test.codex.shutdown_and_wait().await?;
    assert!(
        error.to_string().contains("Output token limit reached"),
        "{error}"
    );
    assert_eq!(log.bodies().len(), 1, "the output cap is terminal");
    Ok(())
}

/// One wire's full image lifecycle: an image+prompt turn whose response
/// calls `update_plan` (proving call/result pairing rides along), the
/// session closes, and a resumed session's next request must carry the same
/// image payload and the same tool pair. The Anthropic cell additionally
/// forks the saved thread before resuming, covering the hot path.
async fn image_resume_cell(wire_api: WireApi) -> Result<()> {
    let server = wiremock::MockServer::start().await;
    let (path, sse_bodies): (&str, Vec<String>) = match wire_api {
        WireApi::Chat => (
            "/v1/chat/completions",
            vec![
                chat_sse(&[
                    chat_tool_call_frame("call_img_plan"),
                    chat_tool_finish_frame(),
                    chat_usage_frame(),
                ]),
                chat_sse(&[
                    chat_text_frame("described", Some("stop")),
                    chat_usage_frame(),
                ]),
                // One extra answer for the forked thread when applicable.
                chat_sse(&[
                    chat_text_frame("after fork", Some("stop")),
                    chat_usage_frame(),
                ]),
                chat_sse(&[
                    chat_text_frame("after resume", Some("stop")),
                    chat_usage_frame(),
                ]),
            ],
        ),
        WireApi::Anthropic => (
            "/v1/messages",
            vec![
                anthropic_sse(vec![anthropic_tool_use("call_img_plan")], "tool_use"),
                anthropic_sse(vec![anthropic_text("described")], "end_turn"),
                anthropic_sse(vec![anthropic_text("after fork")], "end_turn"),
                anthropic_sse(vec![anthropic_text("after resume")], "end_turn"),
            ],
        ),
        WireApi::Responses => (
            "/v1/responses",
            vec![
                responses::sse(vec![
                    responses::ev_response_created("resp_img_tool"),
                    responses::ev_function_call("call_img_plan", "update_plan", &plan_arguments()),
                    responses::ev_completed("resp_img_tool"),
                ]),
                responses::sse(vec![
                    responses::ev_response_created("resp_img_done"),
                    responses::ev_assistant_message("msg_img_done", "described"),
                    responses::ev_completed("resp_img_done"),
                ]),
                responses::sse(vec![
                    responses::ev_response_created("resp_img_resume"),
                    responses::ev_assistant_message("msg_img_resume", "after resume"),
                    responses::ev_completed("resp_img_resume"),
                ]),
            ],
        ),
    };
    let log = mount_sequence(&server, path, sse_bodies).await;

    let provider = rig_image_provider(&server.uri(), wire_api);
    let mut builder = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config({
            let provider = provider.clone();
            move |config| {
                config.model_provider = provider;
                config.update_plan_enabled = true;
            }
        });
    if wire_api == WireApi::Anthropic {
        // The fork API copies a legacy checkpoint; the identity suite
        // established this mode for fork coverage.
        builder = builder.with_history_mode(codex_protocol::protocol::ThreadHistoryMode::Legacy);
    }
    let initial = builder.build_with_auto_env(&server).await?;
    let image_path = initial.config.cwd.join("paste.png");
    write_test_png(&image_path, [12, 34, 56, 255])?;
    submit_image_turn(&initial.codex, "describe this", &image_path).await?;
    let rollout_path = initial.codex.rollout_path().context("rollout path")?;
    initial.codex.shutdown_and_wait().await?;

    let bodies = log.bodies();
    assert_eq!(bodies.len(), 2, "{wire_api:?}: first-session requests");
    let image = image_payload(wire_api, &bodies[0])
        .unwrap_or_else(|| panic!("{wire_api:?}: producing request carries the image"));
    assert!(!image.is_empty());
    assert_media_type(wire_api, &bodies[0]);
    assert!(
        image.len() > 64,
        "{wire_api:?}: the prepared payload is real image bytes, not a placeholder"
    );

    // The Anthropic cell forks the saved checkpoint before resuming: the
    // hot path must replay the same persisted history, image included.
    if wire_api == WireApi::Anthropic {
        let fork = initial
            .thread_manager
            .fork_legacy_thread(
                codex_core::ForkSnapshot::Interrupted,
                codex_core::StartThreadOptions::new(initial.config.clone()),
                rollout_path.clone(),
            )
            .await?
            .thread;
        submit_text_turn_raw(&fork, "after fork").await?;
        assert!(
            image_payload(wire_api, &log.bodies()[2]).as_deref() == Some(image.as_str()),
            "the forked thread replays the exact image payload"
        );
        fork.shutdown_and_wait().await?;
    }

    // The resumed session repeats the SAME image bytes and keeps the
    // update_plan call/result pair from the persisted rollout.
    let cwd = initial.config.cwd.clone();
    let home = initial.home.clone();
    let mut resume_builder = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config({
            let provider = provider.clone();
            move |config| {
                config.model_provider = provider;
                config.update_plan_enabled = true;
                config.cwd = cwd;
            }
        });
    if let Some(exec_server_url) = initial.executor_environment().exec_server_url() {
        resume_builder = resume_builder.with_exec_server_url(exec_server_url);
    }
    let resumed = resume_builder
        .resume(&server, home, rollout_path.clone())
        .await?;
    submit_text_turn_raw(&resumed.codex, "after resume").await?;
    resumed.codex.shutdown_and_wait().await?;

    let bodies = log.bodies();
    let final_request = match wire_api {
        WireApi::Anthropic => &bodies[3],
        WireApi::Chat => &bodies[2],
        WireApi::Responses => &bodies[2],
    };
    assert_eq!(
        image_payload(wire_api, final_request).as_deref(),
        Some(image.as_str()),
        "{wire_api:?}: the resumed request replays the exact image payload"
    );
    let encoded = final_request.to_string();
    match wire_api {
        WireApi::Chat => {
            assert!(
                encoded.contains("\"tool_call_id\":\"call_img_plan\"")
                    && encoded.contains("\"id\":\"call_img_plan\""),
                "chat call/result pair survives resume: {encoded}"
            );
        }
        WireApi::Anthropic => {
            assert!(
                encoded.contains("\"tool_use_id\":\"call_img_plan\"")
                    && encoded.contains("\"id\":\"call_img_plan\""),
                "anthropic call/result pair survives resume: {encoded}"
            );
        }
        WireApi::Responses => {
            let call_pair = final_request["input"]
                .as_array()
                .context("responses input")?
                .iter()
                .filter(|item| item["call_id"] == "call_img_plan")
                .count();
            assert_eq!(call_pair, 2, "responses call/result pair survives resume");
        }
    }
    // The image never degrades into a text copy of its bytes. This is a
    // data-URL check: on Anthropic the base64 payload is the typed
    // `source.data` value itself, so only the URL-bearing wires can show
    // the failure mode of the payload leaking into a text field.
    if wire_api != WireApi::Anthropic {
        let bare_base64 = image
            .strip_prefix("data:image/png;base64,")
            .unwrap_or(image.as_str())
            .to_string();
        let texts = final_request.to_string();
        assert!(
            !texts.contains(&format!("\"{bare_base64}\"")),
            "{wire_api:?}: the payload must ride the typed image field, not a text copy"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_image_history_survives_cold_resume() -> Result<()> {
    skip_if_no_network!(Ok(()));
    image_resume_cell(WireApi::Chat).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_image_history_survives_cold_resume() -> Result<()> {
    skip_if_no_network!(Ok(()));
    image_resume_cell(WireApi::Anthropic).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_image_history_survives_cold_resume() -> Result<()> {
    skip_if_no_network!(Ok(()));
    image_resume_cell(WireApi::Responses).await
}

/// Local compaction summarizes into text and drops media by design; the
/// compaction request itself must go through the rig Chat wire, and the
/// post-compaction turn must continue without the image bytes while the
/// rollout stays append-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_local_compact_with_image_history_continues() -> Result<()> {
    use codex_core::compact::SUMMARIZATION_PROMPT;
    use codex_protocol::protocol::Op;

    let server = wiremock::MockServer::start().await;
    let log = mount_sequence(
        &server,
        "/v1/chat/completions",
        vec![
            chat_sse(&[
                chat_text_frame("described", Some("stop")),
                chat_usage_frame(),
            ]),
            chat_sse(&[
                chat_text_frame("COMPACT SUMMARY", Some("stop")),
                chat_usage_frame(),
            ]),
            chat_sse(&[
                chat_text_frame("followed up", Some("stop")),
                chat_usage_frame(),
            ]),
        ],
    )
    .await;
    let provider = rig_image_provider(&server.uri(), WireApi::Chat);
    let test = test_codex()
        .with_auth_manager(codex_login::test_support::auth_manager_from_optional_auth(
            /*auth*/ None,
        ))
        .with_config(move |config| {
            config.model_provider = provider;
            config.compact_prompt = Some(SUMMARIZATION_PROMPT.to_string());
        })
        .build_with_auto_env(&server)
        .await?;
    let image_path = test.config.cwd.join("paste.png");
    write_test_png(&image_path, [98, 76, 54, 255])?;
    let rollout_path = test.codex.rollout_path().context("rollout path")?;
    submit_image_turn(&test.codex, "describe this", &image_path).await?;
    let before_compact = std::fs::read_to_string(&rollout_path)?;

    test.codex.submit(Op::Compact).await?;
    wait_turn_complete(&test.codex).await?;
    submit_text_turn_raw(&test.codex, "follow up").await?;
    test.codex.shutdown_and_wait().await?;

    let bodies = log.bodies();
    assert_eq!(bodies.len(), 3, "image turn, compaction, follow-up");
    let image = image_payload(WireApi::Chat, &bodies[0])
        .context("the producing request carries the image")?;
    assert_media_type(WireApi::Chat, &bodies[0]);
    assert!(
        bodies[1]
            .to_string()
            .contains("CONTEXT CHECKPOINT COMPACTION"),
        "the compaction request goes through the rig Chat wire"
    );
    let follow_up = bodies[2].to_string();
    assert!(
        follow_up.contains("follow up"),
        "the follow-up turn continues after compaction"
    );
    assert!(
        !follow_up.contains(&image),
        "local compaction drops media from the retained history"
    );
    let after_compact = std::fs::read_to_string(&rollout_path)?;
    assert!(
        after_compact.starts_with(&before_compact),
        "compaction appends; it must not rewrite the saved rollout bytes"
    );
    Ok(())
}
