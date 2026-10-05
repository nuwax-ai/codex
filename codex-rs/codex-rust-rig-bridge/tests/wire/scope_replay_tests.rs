//! Public stream entry points bind replay to the credentials actually sent.
use super::error_tests::provider;
use super::identity_replay_tests::response;
use super::support;
use codex_api::AuthProvider;
use codex_api::ResponseEvent;
use codex_api::SharedAuthProvider;
use codex_protocol::models::ResponseItem;
use codex_rust_rig_bridge::RigProtocol;
use codex_rust_rig_bridge::stream_via_rig;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Clone, Copy)]
enum CredentialProof {
    Immutable,
    Dynamic,
}

struct CountingAuth {
    token: &'static str,
    proof: CredentialProof,
    resolutions: Arc<AtomicUsize>,
}

impl AuthProvider for CountingAuth {
    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static(self.token),
        );
    }

    fn immutable_credential_headers(&self) -> Option<http::HeaderMap> {
        match self.proof {
            CredentialProof::Immutable => Some(self.to_auth_headers()),
            CredentialProof::Dynamic => None,
        }
    }

    fn resolve_auth_headers(&self) -> codex_api::AuthHeadersFuture<'_> {
        Box::pin(async {
            self.resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(self.to_auth_headers())
        })
    }
}

#[tokio::test]
async fn public_stream_replays_only_proven_unchanged_request_credentials() {
    #[derive(Clone, Copy, Debug)]
    enum Change {
        Telemetry,
        UrlQuery,
        ConfigQuery,
        ProviderHeader,
        ExtraHeader,
        Auth,
        MaskedProviderHeader,
        Dynamic,
    }
    for change in [
        Change::Telemetry,
        Change::UrlQuery,
        Change::ConfigQuery,
        Change::ProviderHeader,
        Change::ExtraHeader,
        Change::Auth,
        Change::MaskedProviderHeader,
        Change::Dynamic,
    ] {
        let blocks = vec![
            json!({"type":"thinking","thinking":"thought","signature":"SIGNED"}),
            json!({"type":"server_tool_use","id":"search","name":"web_search","input":{"query":"q"}}),
            json!({"type":"web_search_tool_result","tool_use_id":"search","content":[{"type":"web_search_result","url":"https://example.com","encrypted_content":"ENC"}]}),
            json!({"type":"text","text":"answer"}),
        ];
        let payload = response("first", blocks.clone(), "end_turn");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut provider = provider(listener.local_addr().unwrap());
        provider.base_url.push_str("?tenant=first");
        provider.query_params = Some([("sessionkey".into(), "first".into())].into());
        provider
            .headers
            .insert("x-vendor-session", http::HeaderValue::from_static("first"));
        let server = tokio::spawn(async move {
            let first = support::serve_payload(&listener, &payload).await;
            let second = support::serve_payload(&listener, support::ANTHROPIC_SSE).await;
            vec![first, second]
        });
        let resolutions = Arc::new(AtomicUsize::new(0));
        let proof = match change {
            Change::Dynamic => CredentialProof::Dynamic,
            Change::Telemetry
            | Change::UrlQuery
            | Change::ConfigQuery
            | Change::ProviderHeader
            | Change::ExtraHeader
            | Change::Auth
            | Change::MaskedProviderHeader => CredentialProof::Immutable,
        };
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "x-codex-inference-call-id",
            http::HeaderValue::from_static("first"),
        );
        if matches!(change, Change::ExtraHeader | Change::MaskedProviderHeader) {
            headers.insert(
                "x-vendor-session",
                http::HeaderValue::from_static("override"),
            );
        }
        let mut request = support::request(vec![support::user()]);
        support::set_tools(&mut request, json!([{"type":"web_search"}]));
        let mut saved = Vec::new();
        for attempt in 0..2 {
            let token = if attempt == 1 && matches!(change, Change::Auth) {
                "Bearer rotated"
            } else {
                "Bearer stable"
            };
            let auth: SharedAuthProvider = Arc::new(CountingAuth {
                token,
                proof,
                resolutions: Arc::clone(&resolutions),
            });
            let mut stream = stream_via_rig(
                &request,
                &provider,
                &auth,
                headers.clone(),
                RigProtocol::Anthropic,
                Duration::from_secs(5),
            )
            .await
            .unwrap();
            let mut completed = 0;
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                if matches!(event, ResponseEvent::Completed { .. }) {
                    completed += 1;
                }
                if attempt == 0
                    && let ResponseEvent::OutputItemDone(item) = event
                {
                    saved.push(item);
                }
            }
            assert_eq!(completed, 1, "attempt={attempt}, {change:?}");
            if attempt == 0 {
                request.input.extend(saved.iter().cloned());
                request
                    .input
                    .push(serde_json::from_value(support::user()).unwrap());
                headers.insert(
                    "x-codex-inference-call-id",
                    http::HeaderValue::from_static("second"),
                );
                match change {
                    Change::UrlQuery => {
                        provider.base_url =
                            provider.base_url.replace("tenant=first", "tenant=second")
                    }
                    Change::ConfigQuery => {
                        provider.query_params =
                            Some([("sessionkey".into(), "second".into())].into())
                    }
                    Change::ProviderHeader | Change::MaskedProviderHeader => {
                        provider
                            .headers
                            .insert("x-vendor-session", http::HeaderValue::from_static("second"));
                    }
                    Change::ExtraHeader => {
                        headers
                            .insert("x-vendor-session", http::HeaderValue::from_static("second"));
                    }
                    Change::Telemetry | Change::Auth | Change::Dynamic => {}
                }
            }
        }
        let wire = server.await.unwrap();
        let expected = if matches!(change, Change::Telemetry | Change::MaskedProviderHeader) {
            blocks
        } else {
            vec![json!({"type":"text","text":"answer"})]
        };
        assert_eq!(
            wire[1]["body"]["messages"],
            json!([
                {"role":"user","content":[{"type":"text","text":"hello"}]},
                {"role":"assistant","content":expected},
                {"role":"user","content":[{"type":"text","text":"hello"}]},
            ]),
            "{change:?}"
        );
        assert_eq!(
            resolutions.load(Ordering::SeqCst),
            2,
            "one resolution per attempt: {change:?}"
        );
        assert_eq!(
            &request.input[1..request.input.len() - 1],
            saved.as_slice(),
            "saved history: {change:?}"
        );
        match change {
            Change::Auth => assert!(
                wire[1]["headers"]
                    .as_str()
                    .unwrap()
                    .contains("x-api-key: rotated")
            ),
            Change::UrlQuery => assert!(
                wire[1]["request_line"]
                    .as_str()
                    .unwrap()
                    .contains("tenant=second")
            ),
            Change::ConfigQuery => assert!(
                wire[1]["request_line"]
                    .as_str()
                    .unwrap()
                    .contains("sessionkey=second")
            ),
            Change::ProviderHeader | Change::ExtraHeader => assert!(
                wire[1]["headers"]
                    .as_str()
                    .unwrap()
                    .contains("x-vendor-session: second")
            ),
            Change::MaskedProviderHeader => assert!(
                wire[1]["headers"]
                    .as_str()
                    .unwrap()
                    .contains("x-vendor-session: override")
            ),
            Change::Telemetry | Change::Dynamic => {}
        }
    }
}

#[tokio::test]
async fn unproven_public_scope_is_shared_only_by_one_pause_chain() {
    let blocks: Vec<_> = ["first", "second"]
        .into_iter()
        .map(|id| {
            vec![
                json!({"type":"thinking","thinking":id,"signature":format!("signed-{id}")}),
                json!({"type":"server_tool_use","id":id,"name":"web_search","input":{"query":id}}),
                json!({"type":"web_search_tool_result","tool_use_id":id,"content":[]}),
                json!({"type":"text","text":id}),
            ]
        })
        .collect();
    let (address, server) = support::sequence_server(vec![
        response("first", blocks[0].clone(), "pause_turn"),
        response("second", blocks[1].clone(), "end_turn"),
        support::ANTHROPIC_SSE.into(),
    ])
    .await;
    let provider = provider(address);
    let resolutions = Arc::new(AtomicUsize::new(0));
    let auth: SharedAuthProvider = Arc::new(CountingAuth {
        token: "Bearer stable",
        proof: CredentialProof::Dynamic,
        resolutions: Arc::clone(&resolutions),
    });
    let mut request = support::request(vec![support::user()]);
    support::set_tools(&mut request, json!([{"type":"web_search"}]));
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
    let mut saved = Vec::new();
    while let Some(event) = stream.next().await {
        if let ResponseEvent::OutputItemDone(item) = event.unwrap() {
            saved.push(item);
        }
    }
    let sources: Vec<_> = saved
        .iter()
        .filter_map(|item| match item {
            ResponseItem::WebSearchCall {
                wire_blocks: Some(payload),
                ..
            } => Some(payload["source"].as_str().unwrap().to_string()),
            _ => None,
        })
        .collect();
    let source = sources.first().expect("captured search source").clone();
    assert_eq!(sources, vec![source.clone(), source]);
    request.input.extend(saved);
    request
        .input
        .push(serde_json::from_value(support::user()).unwrap());
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
    let bodies = server.await.unwrap();
    let user = json!({"role":"user","content":[{"type":"text","text":"hello"}]});
    assert_eq!(
        bodies[1]["messages"],
        json!([
            user, {"role":"assistant","content":blocks[0]},
        ])
    );
    assert_eq!(
        bodies[2]["messages"],
        json!([
            user.clone(),
            {"role":"assistant","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]},
            user,
        ])
    );
    assert_eq!(resolutions.load(Ordering::SeqCst), 3);
}
