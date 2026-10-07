//! Same-process Core public-path replay of signed thinking, completed hosted
//! search ciphertext and encrypted citations. Query rotations keep the same
//! endpoint identity and vary only private values, including configured query
//! parameters. Complete HTTP requests and persisted provenance establish which
//! source changed; exact message comparisons establish replay and degradation.
//! Credential-only rotation is covered by the credential-instance sibling suite.

use super::*;
use codex_history::ModelOutputProvenance;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::collections::HashMap;

enum Rotation {
    Identical,
    UrlQuery {
        before: &'static str,
        after: &'static str,
    },
    QueryParams {
        before: &'static str,
        after: &'static str,
    },
    HeaderValue(&'static str),
}

#[derive(Clone, Copy)]
enum Replay {
    Preserve,
    Degrade,
}

fn rotation_provider(server_uri: &str) -> ModelProviderInfo {
    let mut provider = anthropic_provider(server_uri);
    provider.requires_openai_auth = true;
    provider.http_headers = Some(HashMap::from([(
        "x-rotation-extra".to_string(),
        "value-a".into(),
    )]));
    provider
}

fn thinking_block() -> Value {
    json!({"type":"thinking", "thinking":"visible thought",
        "signature":"SIGNED_ROTATION_SOURCE"})
}

fn citation_block() -> Value {
    json!({"type":"text", "text":"visible answer", "citations":[{
        "type":"web_search_result_location", "url":"https://example.com", "title":"example",
        "cited_text":"finding", "encrypted_index":"INDEX_ROTATION_SOURCE"}]})
}

fn producing_response() -> String {
    let signed_frame = format!(
        "event: content_block_start\ndata: {}\n\n",
        json!({"type":"content_block_start", "index":0, "content_block":thinking_block()})
    );
    let mut cited_start = citation_block();
    cited_start["text"] = json!("");
    let cited_frame = format!(
        "event: content_block_start\ndata: {}\n\nevent: content_block_delta\ndata: {}\n\n",
        json!({"type":"content_block_start", "index":3, "content_block":cited_start}),
        json!({"type":"content_block_delta", "index":3, "delta":{"type":"text_delta","text":"visible answer"}})
    );
    format!(
        "{}{signed_frame}{}{}{}{}{}{cited_frame}{}{}",
        message_start("rotation-produce"),
        block_stop(0),
        server_tool_use_block(1, "srvu_rotation", "query"),
        block_stop(1),
        search_result_block(2, "srvu_rotation", "ENCRYPTED_ROTATION_SOURCE"),
        block_stop(2),
        block_stop(3),
        message_delta("end_turn")
    )
}

fn following_response() -> String {
    format!(
        "{}{}{}",
        message_start("rotation-follow"),
        plain_text_block(0, "followed"),
        message_delta("end_turn")
    )
}

struct ExpectedRequest<'a> {
    query: Option<&'a str>,
    extra_header: &'a str,
}

fn assert_request(request: &wiremock::Request, expected: ExpectedRequest<'_>) -> Result<Value> {
    let body: Value = serde_json::from_slice(&request.body)?;
    assert_eq!(
        (
            request.method.as_str(),
            request.url.path(),
            request.url.query(),
            request
                .headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            request
                .headers
                .get("x-rotation-extra")
                .and_then(|value| value.to_str().ok()),
            body["model"].as_str(),
        ),
        (
            "POST",
            "/v1/messages",
            expected.query,
            Some("rotation-key"),
            Some(expected.extra_header),
            Some("gpt-5.5")
        ),
        "the actual wire must match the source's path, query, credentials, headers and model"
    );
    Ok(body)
}

fn saved_source(rollout: &str) -> Result<ModelOutputProvenance> {
    let sources: Vec<_> = rollout
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
    let source = sources
        .first()
        .context("persisted model output provenance")?;
    assert!(sources.iter().all(|candidate| candidate == source));
    assert_eq!(
        (
            source.wire_protocol.as_str(),
            source.bridge.as_deref(),
            source.provider.as_deref(),
            source.model.as_deref()
        ),
        (
            "anthropic",
            Some("rig"),
            Some("anthropic-hosted-test"),
            Some("gpt-5.5")
        ),
        "saved provenance must describe the actual model request"
    );
    assert_eq!(
        source.auth_domain_kind.as_deref(),
        Some("credentialInstance")
    );
    assert!(source.auth_domain.is_some());
    let encoded = serde_json::to_string(&sources)?;
    for private in [
        "rotation-key",
        "value-a",
        "value-b",
        "private-one",
        "private-two",
        "private-three",
    ] {
        assert!(
            !encoded.contains(private),
            "credential/query/header bytes entered provenance"
        );
    }
    Ok(source.clone())
}

fn assert_replay(before: &Value, after: &Value, replay: Replay, label: &str) -> Result<()> {
    let content = match replay {
        Replay::Preserve => {
            let mut content = vec![thinking_block()];
            content.extend(search_replay_blocks(
                "srvu_rotation",
                "query",
                "ENCRYPTED_ROTATION_SOURCE",
            ));
            content.push(citation_block());
            content
        }
        // Anthropic cannot replay thinking without its signed envelope. The
        // ordinary text survives while search ciphertext and citations degrade.
        Replay::Degrade => vec![json!({"type":"text", "text":"visible answer"})],
    };
    assert_eq!(
        after["messages"],
        expected_follow_up_messages(&before["messages"], content, "after rotation"),
        "{label}: complete history, block order, visible text and user input"
    );
    if matches!(replay, Replay::Degrade) {
        let encoded = serde_json::to_string(after)?;
        for opaque in [
            "SIGNED_ROTATION_SOURCE",
            "ENCRYPTED_ROTATION_SOURCE",
            "INDEX_ROTATION_SOURCE",
        ] {
            assert!(
                !encoded.contains(opaque),
                "{label}: opaque data crossed the rotated source"
            );
        }
    }
    Ok(())
}

async fn run_rotation_cell(rotation: Rotation, replay: Replay, label: &str) -> Result<()> {
    let server = MockServer::start().await;
    mount_anthropic_sequence(&server, vec![producing_response(), following_response()]).await;
    let mut producing_provider = rotation_provider(&server.uri());
    let mut resumed_provider = producing_provider.clone();
    let (before_query, after_query, after_header) = match rotation {
        Rotation::Identical => (None, None, "value-a"),
        Rotation::UrlQuery { before, after } => {
            producing_provider.base_url = Some(format!("{}/v1?{before}", server.uri()));
            resumed_provider.base_url = Some(format!("{}/v1?{after}", server.uri()));
            (Some(before.to_string()), Some(after.to_string()), "value-a")
        }
        Rotation::QueryParams { before, after } => {
            producing_provider.query_params =
                Some(HashMap::from([("sessionkey".into(), before.into())]));
            resumed_provider.query_params =
                Some(HashMap::from([("sessionkey".into(), after.into())]));
            (
                Some(format!("sessionkey={before}")),
                Some(format!("sessionkey={after}")),
                "value-a",
            )
        }
        Rotation::HeaderValue(value) => {
            resumed_provider.http_headers =
                Some(HashMap::from([("x-rotation-extra".into(), value.into())]));
            (None, None, value)
        }
    };
    let initial = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("rotation-key"))
        .with_config(move |config| config.model_provider = producing_provider)
        .build_with_auto_env(&server)
        .await?;
    initial.submit_text_turn("search").await?;
    let rollout = initial
        .codex
        .rollout_path()
        .context("rotation rollout path")?;
    initial.codex.shutdown_and_wait().await?;
    let rollout_before = std::fs::read_to_string(&rollout)?;
    for opaque in [
        "SIGNED_ROTATION_SOURCE",
        "ENCRYPTED_ROTATION_SOURCE",
        "INDEX_ROTATION_SOURCE",
    ] {
        assert!(
            rollout_before.contains(opaque),
            "{label}: producer must persist each opaque payload"
        );
    }
    let source_before = saved_source(&rollout_before)?;
    let cwd = initial.config.cwd.clone();
    let resumed = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("rotation-key"))
        .with_config(move |config| {
            config.model_provider = resumed_provider;
            config.cwd = cwd;
        })
        .resume(&server, initial.home.clone(), rollout.clone())
        .await?;
    resumed.submit_text_turn("after rotation").await?;
    resumed.codex.shutdown_and_wait().await?;
    let rollout_after = std::fs::read_to_string(&rollout)?;
    let appended = rollout_after
        .strip_prefix(&rollout_before)
        .context("resume must append without changing any existing rollout bytes")?;
    let source_after = saved_source(appended)?;
    assert!(source_before.endpoint_identity.is_some());
    assert_eq!(
        source_before.endpoint_identity, source_after.endpoint_identity,
        "{label}: query/header rotation must not be hidden by a different endpoint identity"
    );
    let mut expected_source = source_before.clone();
    match replay {
        Replay::Preserve => {}
        Replay::Degrade => {
            assert_ne!(
                source_before.auth_domain, source_after.auth_domain,
                "{label}"
            );
            expected_source.auth_domain = source_after.auth_domain.clone();
        }
    }
    assert_eq!(
        source_after, expected_source,
        "{label}: only private scope may change"
    );
    let requests = server
        .received_requests()
        .await
        .context("recorded rotation requests")?;
    assert_eq!(
        requests.len(),
        2,
        "{label}: exactly one producing and one resumed attempt"
    );
    let before = assert_request(
        &requests[0],
        ExpectedRequest {
            query: before_query.as_deref(),
            extra_header: "value-a",
        },
    )?;
    let after = assert_request(
        &requests[1],
        ExpectedRequest {
            query: after_query.as_deref(),
            extra_header: after_header,
        },
    )?;
    assert_replay(&before, &after, replay, label)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_source_rotation_preserves_or_degrades_opaque_per_dimension() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let cells = [
        ("identical source", Rotation::Identical, Replay::Preserve),
        (
            "identical URL query",
            Rotation::UrlQuery {
                before: "sessionkey=private-one",
                after: "sessionkey=private-one",
            },
            Replay::Preserve,
        ),
        (
            "unknown URL query value",
            Rotation::UrlQuery {
                before: "sessionkey=private-one",
                after: "sessionkey=private-two",
            },
            Replay::Degrade,
        ),
        (
            "duplicate URL query value",
            Rotation::UrlQuery {
                before: "a=private-one&a=private-two",
                after: "a=private-one&a=private-three",
            },
            Replay::Degrade,
        ),
        (
            "duplicate URL query order",
            Rotation::UrlQuery {
                before: "a=private-one&a=private-two",
                after: "a=private-two&a=private-one",
            },
            Replay::Degrade,
        ),
        (
            "empty URL query value",
            Rotation::UrlQuery {
                before: "q=private-one",
                after: "q=",
            },
            Replay::Degrade,
        ),
        (
            "identical configured query",
            Rotation::QueryParams {
                before: "private-one",
                after: "private-one",
            },
            Replay::Preserve,
        ),
        (
            "configured query value",
            Rotation::QueryParams {
                before: "private-one",
                after: "private-two",
            },
            Replay::Degrade,
        ),
        (
            "empty configured query value",
            Rotation::QueryParams {
                before: "private-one",
                after: "",
            },
            Replay::Degrade,
        ),
        (
            "HTTP header value",
            Rotation::HeaderValue("value-b"),
            Replay::Degrade,
        ),
    ];
    for (label, rotation, replay) in cells {
        run_rotation_cell(rotation, replay, label).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_endpoint_rotation_degrades_opaque_and_keeps_visible() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let producing_server = MockServer::start().await;
    let follow_server = MockServer::start().await;
    mount_anthropic_sequence(&producing_server, vec![producing_response()]).await;
    mount_anthropic_sequence(&follow_server, vec![following_response()]).await;
    let provider = rotation_provider(&producing_server.uri());
    let mut rotated = provider.clone();
    rotated.base_url = Some(format!("{}/v1", follow_server.uri()));
    let initial = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("rotation-key"))
        .with_config(move |config| config.model_provider = provider)
        .build_with_auto_env(&producing_server)
        .await?;
    initial.submit_text_turn("search").await?;
    let rollout = initial
        .codex
        .rollout_path()
        .context("endpoint rotation rollout")?;
    initial.codex.shutdown_and_wait().await?;
    let rollout_before = std::fs::read_to_string(&rollout)?;
    let source_before = saved_source(&rollout_before)?;
    for opaque in [
        "SIGNED_ROTATION_SOURCE",
        "ENCRYPTED_ROTATION_SOURCE",
        "INDEX_ROTATION_SOURCE",
    ] {
        assert!(
            rollout_before.contains(opaque),
            "endpoint producer must persist each opaque payload"
        );
    }
    let cwd = initial.config.cwd.clone();
    let resumed = test_codex()
        .with_auth(codex_login::CodexAuth::from_api_key("rotation-key"))
        .with_config(move |config| {
            config.model_provider = rotated;
            config.cwd = cwd;
        })
        .resume(&follow_server, initial.home.clone(), rollout.clone())
        .await?;
    resumed.submit_text_turn("after rotation").await?;
    resumed.codex.shutdown_and_wait().await?;
    let rollout_after = std::fs::read_to_string(&rollout)?;
    let appended = rollout_after
        .strip_prefix(&rollout_before)
        .context("endpoint rotation must only append to the rollout")?;
    let source_after = saved_source(appended)?;
    assert!(source_before.endpoint_identity.is_some());
    assert!(source_after.endpoint_identity.is_some());
    assert_ne!(
        source_before.endpoint_identity,
        source_after.endpoint_identity
    );
    assert_ne!(source_before.auth_domain, source_after.auth_domain);
    let mut expected_source = source_before;
    expected_source.endpoint_identity = source_after.endpoint_identity.clone();
    expected_source.auth_domain = source_after.auth_domain.clone();
    assert_eq!(
        source_after, expected_source,
        "only the endpoint and its private instance may change"
    );
    let producing_requests = producing_server
        .received_requests()
        .await
        .context("producing endpoint requests")?;
    let follow_requests = follow_server
        .received_requests()
        .await
        .context("rotated endpoint requests")?;
    assert_eq!(
        (producing_requests.len(), follow_requests.len()),
        (1, 1),
        "exactly one attempt on each endpoint"
    );
    let before = assert_request(
        &producing_requests[0],
        ExpectedRequest {
            query: None,
            extra_header: "value-a",
        },
    )?;
    let after = assert_request(
        &follow_requests[0],
        ExpectedRequest {
            query: None,
            extra_header: "value-a",
        },
    )?;
    assert_replay(&before, &after, Replay::Degrade, "endpoint rotation")
}
