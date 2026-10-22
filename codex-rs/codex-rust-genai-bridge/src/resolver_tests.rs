use super::*;
use codex_api::AuthProvider;
use codex_api::RetryConfig;
use futures::StreamExt;
use std::collections::BTreeMap;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

struct FixtureAuth;

impl AuthProvider for FixtureAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<(), AuthError> {
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer fixture-key"),
        );
        Ok(())
    }
}

struct UnavailableAuth;

impl AuthProvider for UnavailableAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<(), AuthError> {
        // A provider may have populated some headers before discovering it
        // cannot complete auth. Neither caller may treat those as success.
        headers.insert("x-partial-auth", http::HeaderValue::from_static("fixture"));
        Err(AuthError::Transient(
            "fixture credentials unavailable".into(),
        ))
    }
}

fn provider() -> Provider {
    Provider {
        name: "auth failure fixture".into(),
        base_url: "http://127.0.0.1:9/v1".into(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: RetryConfig {
            max_attempts: 1,
            base_delay: Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: Duration::from_secs(1),
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

#[test]
fn auth_failure_is_not_an_unauthenticated_genai_client() {
    let auth: SharedAuthProvider = Arc::new(UnavailableAuth);
    let result = build_genai_client(&provider(), &auth, AdapterKind::OpenAI);
    assert!(matches!(
        result,
        Err(AuthError::Transient(message)) if message == "fixture credentials unavailable"
    ));
}

#[test]
fn auth_failure_is_not_a_partial_extra_header_map() {
    let auth: SharedAuthProvider = Arc::new(UnavailableAuth);
    let result = build_extra_headers(&provider(), &auth, &HeaderMap::new());
    assert!(matches!(
        result,
        Err(AuthError::Transient(message)) if message == "fixture credentials unavailable"
    ));
}

#[test]
fn openai_request_preserves_base_path_and_encoded_query_parameters() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    for base_path in ["/v1", "/v1/"] {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let address = listener.local_addr().expect("listener address");
        let captured = std::thread::spawn(move || capture_request(listener));
        let mut provider = provider();
        provider.base_url = format!("http://{address}{base_path}?existing=a%2Fb%26c&empty=");
        provider.query_params = Some(
            [
                ("tenant".to_string(), "a&b /+?#=文字".to_string()),
                ("api-version".to_string(), "2026-01-01".to_string()),
            ]
            .into_iter()
            .collect(),
        );
        let auth: SharedAuthProvider = Arc::new(FixtureAuth);
        let client =
            build_genai_client(&provider, &auth, AdapterKind::OpenAI).expect("configured client");
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), async {
                let mut response = client
                    .exec_chat_stream(
                        "fixture-model",
                        genai::chat::ChatRequest::new(vec![genai::chat::ChatMessage::user(
                            "hello",
                        )]),
                        None,
                    )
                    .await
                    .expect("loopback response stream");
                // genai starts the HTTP request lazily when the stream is polled.
                while let Some(event) = response.stream.next().await {
                    event.expect("loopback SSE event");
                }
            })
            .await
            .expect("bounded loopback request");
        });
        let request = captured.join().expect("capture thread");
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("HTTP request target");
        let url =
            reqwest::Url::parse(&format!("http://fixture.invalid{target}")).expect("captured URL");
        assert_eq!(url.path(), "/v1/chat/completions", "{base_path}");
        let query: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query,
            BTreeMap::from([
                ("existing".to_string(), "a/b&c".to_string()),
                ("empty".to_string(), String::new()),
                ("tenant".to_string(), "a&b /+?#=文字".to_string()),
                ("api-version".to_string(), "2026-01-01".to_string()),
            ]),
            "{base_path}"
        );
    }
}

fn capture_request(listener: TcpListener) -> String {
    listener.set_nonblocking(true).expect("nonblocking accept");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "loopback client did not connect");
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("loopback accept failed: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("bounded request read");
    let mut request = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk).expect("request bytes");
        assert_ne!(read, 0, "client closed before sending a complete request");
        request.extend_from_slice(&chunk[..read]);
        if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().expect("content length"))
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 14\r\nConnection: close\r\n\r\ndata: [DONE]\n\n",
        )
        .expect("fixture response");
    String::from_utf8(request).expect("UTF-8 request")
}

#[test]
fn anthropic_query_parameters_are_rejected_without_exposing_values() {
    let auth: SharedAuthProvider = Arc::new(FixtureAuth);
    for query_in_base in [false, true] {
        let mut provider = provider();
        if query_in_base {
            provider.base_url = "https://fixture.invalid/v1?api-key=private-query-value".into();
        } else {
            provider.query_params = Some(
                [("api-key".to_string(), "private-query-value".to_string())]
                    .into_iter()
                    .collect(),
            );
        }
        let result = build_genai_client(&provider, &auth, AdapterKind::Anthropic);
        let Err(AuthError::Build(message)) = result else {
            panic!("Anthropic query parameters must fail before a request is sent");
        };
        assert_eq!(
            message,
            "the genai Anthropic bridge does not support endpoint query parameters; use the rig bridge"
        );
    }
}

#[test]
fn anthropic_without_query_parameters_still_builds() {
    let auth: SharedAuthProvider = Arc::new(FixtureAuth);
    assert!(build_genai_client(&provider(), &auth, AdapterKind::Anthropic).is_ok());
}

struct HeaderMapAuth(HeaderMap);

impl AuthProvider for HeaderMapAuth {
    fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<(), AuthError> {
        for (name, value) in &self.0 {
            headers.append(name.clone(), value.clone());
        }
        Ok(())
    }
}

#[test]
fn secondary_auth_header_equal_to_authorization_is_preserved() {
    let value = http::HeaderValue::from_static("Bearer fixture-key");
    let mut auth_headers = HeaderMap::new();
    auth_headers.insert(http::header::AUTHORIZATION, value.clone());
    auth_headers.insert("x-gateway-token", value);
    let auth: SharedAuthProvider = Arc::new(HeaderMapAuth(auth_headers));
    let headers =
        build_extra_headers(&provider(), &auth, &HeaderMap::new()).expect("representable headers");
    assert_eq!(
        headers.into_iter().collect::<BTreeMap<_, _>>(),
        BTreeMap::from([(
            "x-gateway-token".to_string(),
            "Bearer fixture-key".to_string()
        )])
    );
}

#[test]
fn unrepresentable_header_values_are_rejected_in_every_layer() {
    for bad_layer in 0..3 {
        for multiple_values in [false, true] {
            let mut layers = [HeaderMap::new(), HeaderMap::new(), HeaderMap::new()];
            let expected_error = if multiple_values {
                layers[bad_layer].append(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_static("private-first-value"),
                );
                layers[bad_layer].append(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_static("private-second-value"),
                );
                "the genai bridge cannot represent multiple values for one header name"
            } else {
                layers[bad_layer].insert(
                    http::header::AUTHORIZATION,
                    http::HeaderValue::from_bytes(b"private-\x80-value")
                        .expect("opaque HTTP header value"),
                );
                "the genai bridge cannot represent a non-text header value"
            };
            let [provider_headers, auth_headers, extra_headers] = layers;
            let mut provider = provider();
            provider.headers = provider_headers;
            let auth: SharedAuthProvider = Arc::new(HeaderMapAuth(auth_headers));
            let result = build_extra_headers(&provider, &auth, &extra_headers);
            assert!(
                matches!(result, Err(AuthError::Build(message)) if message == expected_error),
                "layer {bad_layer}, multiple_values {multiple_values}"
            );
        }
    }
}

#[test]
fn representable_headers_keep_provider_auth_extra_precedence() {
    let mut provider = provider();
    provider.headers = HeaderMap::from_iter([
        (
            http::HeaderName::from_static("x-shared"),
            http::HeaderValue::from_static("provider"),
        ),
        (
            http::HeaderName::from_static("x-provider-only"),
            http::HeaderValue::from_static("provider"),
        ),
        (
            http::HeaderName::from_static("x-auth-wins"),
            http::HeaderValue::from_static("provider"),
        ),
    ]);
    let auth: SharedAuthProvider = Arc::new(HeaderMapAuth(HeaderMap::from_iter([
        (
            http::HeaderName::from_static("x-shared"),
            http::HeaderValue::from_static("auth"),
        ),
        (
            http::HeaderName::from_static("x-auth-only"),
            http::HeaderValue::from_static("auth"),
        ),
        (
            http::HeaderName::from_static("x-auth-wins"),
            http::HeaderValue::from_static("auth"),
        ),
    ])));
    let extra_headers = HeaderMap::from_iter([
        (
            http::HeaderName::from_static("x-shared"),
            http::HeaderValue::from_static("extra"),
        ),
        (
            http::HeaderName::from_static("x-extra-only"),
            http::HeaderValue::from_static("extra"),
        ),
    ]);
    let headers = build_extra_headers(&provider, &auth, &extra_headers)
        .expect("representable layered headers");
    assert_eq!(
        headers.into_iter().collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            ("x-shared".to_string(), "extra".to_string()),
            ("x-provider-only".to_string(), "provider".to_string()),
            ("x-auth-only".to_string(), "auth".to_string()),
            ("x-auth-wins".to_string(), "auth".to_string()),
            ("x-extra-only".to_string(), "extra".to_string()),
        ])
    );
}
