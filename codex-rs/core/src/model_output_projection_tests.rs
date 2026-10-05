use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn provider() -> ModelProviderInfo {
    ModelProviderInfo {
        name: "a display name".into(),
        provider_id: Some("provider-key".into()),
        base_url: Some("https://endpoint.test/v1".into()),
        requires_openai_auth: false,
        ..Default::default()
    }
}

struct StaticEmpty;
impl codex_api::AuthProvider for StaticEmpty {
    fn add_auth_headers(&self, _headers: &mut http::HeaderMap) {}
    fn immutable_credential_headers(&self) -> Option<http::HeaderMap> {
        Some(http::HeaderMap::new())
    }
}

struct StaticBearer {
    token: &'static str,
}
impl codex_api::AuthProvider for StaticBearer {
    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_str(self.token).unwrap(),
        );
    }
    fn immutable_credential_headers(&self) -> Option<http::HeaderMap> {
        Some(codex_api::AuthProvider::to_auth_headers(self))
    }
}

struct DynamicBearer;
impl codex_api::AuthProvider for DynamicBearer {
    fn add_auth_headers(&self, headers: &mut http::HeaderMap) {
        headers.insert(
            http::header::AUTHORIZATION,
            http::HeaderValue::from_static("Bearer changing"),
        );
    }
}

fn source() -> ModelOutputProvenance {
    let provider = provider();
    request_source(
        &provider,
        &provider.to_api_provider(None).unwrap(),
        None,
        "model-a",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap()
}

#[test]
fn signed_query_is_an_actual_credential_instance_not_anonymous_routing() {
    let info = provider();
    let mut api = info.to_api_provider(None).expect("provider");
    api.query_params = Some([("X-Amz-Signature".into(), "first-private-signature".into())].into());
    let first = request_source(
        &info,
        &api,
        None,
        "model-a",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .expect("source");
    assert_eq!(
        first.auth_domain_kind.as_deref(),
        Some("credentialInstance")
    );
    api.query_params =
        Some([("X-Amz-Signature".into(), "rotated-private-signature".into())].into());
    let second = request_source(
        &info,
        &api,
        None,
        "model-a",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .expect("source");
    assert_eq!(first.endpoint_identity, second.endpoint_identity);
    assert_ne!(first.auth_domain, second.auth_domain);
}

fn fixture_items() -> Vec<ResponseItem> {
    serde_json::from_value(json!([
        {"type":"reasoning", "id":"rs_opaque", "summary":[{"type":"summary_text", "text":"visible summary"}], "content":[{"type":"reasoning_text", "text":"visible thought"}], "encrypted_content":"opaque"},
        {"type":"compaction", "id":"cmp_opaque", "encrypted_content":"opaque compact"},
        {"type":"web_search_call", "id":"search_opaque", "status":"completed", "action":{"type":"search", "query":"query"}, "wire_blocks":{"encrypted_content":"hosted result"}},
        {"type":"message", "id":"msg_visible", "role":"assistant", "content":[{"type":"output_text", "text":"answer"}]},
        {"type":"function_call", "id":"call_visible", "call_id":"call", "name":"shell", "arguments":"{}"},
        {"type":"function_call_output", "id":"output_visible", "call_id":"call", "output":"tool result"}
    ])).unwrap()
}

fn annotated(items: &[ResponseItem], source: &ModelOutputProvenance) -> Vec<ResponseItemEnvelope> {
    items
        .iter()
        .cloned()
        .map(|item| {
            let mut envelope = ResponseItemEnvelope::new(item);
            envelope
                .metadata
                .get_or_insert_default()
                .model_output_provenance = Some(source.clone());
            envelope
        })
        .collect()
}

#[test]
fn matching_sources_preserve_opaque_and_never_mutate_saved_envelopes() {
    let items = fixture_items();
    let saved = annotated(&items, &source());
    let before = saved.clone();
    let mut projected = items.clone();
    project_input(&mut projected, &sources_for_input(&saved), &source());
    assert_eq!(projected, items);
    assert_eq!(saved, before);
}

#[test]
fn every_source_boundary_and_legacy_unknown_drops_only_opaque_payloads() {
    let items = fixture_items();
    let saved = annotated(&items, &source());
    let mut expected = items.clone();
    if let ResponseItem::Reasoning {
        encrypted_content, ..
    } = &mut expected[0]
    {
        *encrypted_content = None;
    }
    expected.remove(1);
    if let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut expected[1] {
        *wire_blocks = None;
    }
    let mut targets = Vec::new();
    for dimension in [
        "wire", "bridge", "provider", "model", "endpoint", "domain", "proof", "unknown",
    ] {
        let mut target = source();
        match dimension {
            "wire" => target.wire_protocol = "anthropic".into(),
            "bridge" => target.bridge = Some("genai".into()),
            "provider" => target.provider = Some("other-config-key".into()),
            "model" => target.model = Some("other-model".into()),
            "endpoint" => target.endpoint_identity = Some("other-endpoint".into()),
            "domain" => target.auth_domain = Some("other-account".into()),
            "proof" => target.auth_domain_kind = Some("selector".into()),
            "unknown" => target.auth_domain = None,
            _ => unreachable!(),
        }
        targets.push(target);
    }
    for target in targets {
        let mut projected = items.clone();
        project_input(&mut projected, &sources_for_input(&saved), &target);
        assert_eq!(projected, expected);
    }
    let mut legacy = items;
    project_input(&mut legacy, &InputProvenance::new(), &source());
    assert_eq!(legacy, expected);
    assert_eq!(saved, annotated(&fixture_items(), &source()));
}

#[test]
fn item_identity_survives_prompt_insertions_and_duplicate_sources_fail_closed() {
    let items = fixture_items();
    let mut saved = annotated(&items, &source());
    let mut conflicting = saved[0].clone();
    conflicting
        .metadata
        .as_mut()
        .unwrap()
        .model_output_provenance
        .as_mut()
        .unwrap()
        .model = Some("other".into());
    saved.push(conflicting);
    let sources = sources_for_input(&saved);
    let mut projected = items.clone();
    projected.insert(0, serde_json::from_value(json!({"type":"message", "role":"developer", "content":[{"type":"input_text","text":"new instruction"}]})).unwrap());
    project_input(&mut projected, &sources, &source());
    let mut expected = items;
    if let ResponseItem::Reasoning {
        encrypted_content, ..
    } = &mut expected[0]
    {
        *encrypted_content = None;
    }
    expected.insert(0, projected[0].clone());
    assert_eq!(projected, expected);
}

#[test]
fn actual_credential_snapshot_overrides_selector_and_ignores_ambient_account() {
    let mut info = provider();
    info.env_key = Some("KEY_A".into());
    let first = StaticBearer {
        token: "Bearer first-key",
    };
    let a = request_source(
        &info,
        &info.to_api_provider(None).unwrap(),
        None,
        "model",
        &first,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(a.auth_domain_kind.as_deref(), Some("credentialInstance"));
    info.name = "new display".into();
    info.env_key = Some("KEY_B".into());
    assert_eq!(
        request_source(
            &info,
            &info.to_api_provider(None).unwrap(),
            None,
            "model",
            &first,
            None,
            &http::HeaderMap::new(),
        )
        .unwrap(),
        a
    );
    let second = StaticBearer {
        token: "Bearer second-key",
    };
    let b = request_source(
        &info,
        &info.to_api_provider(None).unwrap(),
        None,
        "model",
        &second,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_ne!(a.auth_domain, b.auth_domain);
    assert!(!a.auth_domain.as_ref().unwrap().contains("first-key"));
    let dynamic = request_source(
        &info,
        &info.to_api_provider(None).unwrap(),
        None,
        "model",
        &DynamicBearer,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(dynamic.auth_domain_kind.as_deref(), Some("selector"));
    info.env_key = None;
    let ambient = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let anonymous = request_source(
        &info,
        &info.to_api_provider(None).unwrap(),
        Some(&ambient),
        "model",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(
        (
            anonymous.auth_domain.as_deref(),
            anonymous.auth_domain_kind.as_deref()
        ),
        (Some("anonymous"), Some("anonymous"))
    );
    let unknown = request_source(
        &info,
        &info.to_api_provider(None).unwrap(),
        None,
        "model",
        &DynamicBearer,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(
        (unknown.auth_domain, unknown.auth_domain_kind),
        (None, None)
    );
}

#[test]
fn same_selector_changed_actual_credentials_drop_old_opaque_but_keep_visible_history() {
    let mut info = provider();
    info.env_key = Some("THE_SAME_VARIABLE".into());
    let api = info.to_api_provider(None).unwrap();
    let old = request_source(
        &info,
        &api,
        None,
        "model",
        &StaticBearer {
            token: "Bearer old",
        },
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    let new = request_source(
        &info,
        &api,
        None,
        "model",
        &StaticBearer {
            token: "Bearer new",
        },
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    let input = fixture_items();
    let saved = annotated(&input, &old);
    let mut same = input.clone();
    project_input(&mut same, &sources_for_input(&saved), &old);
    assert_eq!(same, input);
    let mut rotated = input.clone();
    project_input(&mut rotated, &sources_for_input(&saved), &new);
    let mut expected = input;
    if let ResponseItem::Reasoning {
        encrypted_content, ..
    } = &mut expected[0]
    {
        *encrypted_content = None;
    }
    expected.remove(1);
    if let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut expected[1] {
        *wire_blocks = None;
    }
    assert_eq!(rotated, expected);
}

#[test]
fn unchanged_auth_selector_without_account_proof_still_drops_opaque() {
    let mut selector = source();
    selector.auth_domain = Some("selector-v1:KEY_A".into());
    selector.auth_domain_kind = Some("selector".into());
    let items = fixture_items();
    let saved = annotated(&items, &selector);
    let mut projected = items.clone();
    project_input(&mut projected, &sources_for_input(&saved), &selector);
    let mut expected = items;
    if let ResponseItem::Reasoning {
        encrypted_content, ..
    } = &mut expected[0]
    {
        *encrypted_content = None;
    }
    expected.remove(1);
    if let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut expected[1] {
        *wire_blocks = None;
    }
    assert_eq!(projected, expected);
    // Visible unsigned hosted pairs may use the weaker configuration selector.
    let mut plain = fixture_items();
    if let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut plain[2] {
        *wire_blocks = Some(
            json!({"blocks":[{"type":"server_tool_use","id":"s","name":"web_search","input":{}},
            {"type":"web_search_tool_result","tool_use_id":"s","content":[{"type":"text","text":"plain result"}]}]}),
        );
    }
    let saved = annotated(&plain, &selector);
    let expected_search = plain[2].clone();
    project_input(&mut plain, &sources_for_input(&saved), &selector);
    assert_eq!(plain[1], expected_search);
}

#[test]
fn selector_v3_citation_ciphertext_drops_while_visible_message_survives() {
    for field in ["encrypted_index", "encryptedIndex"] {
        let mut selector = source();
        selector.auth_domain = Some("selector-v1:KEY_A".into());
        selector.auth_domain_kind = Some("selector".into());
        let mut citation = json!({"type":"web_search_result_location", "url":"https://example.com",
            "title":"example", "cited_text":"finding"});
        citation[field] = "opaque-citation-index".into();
        // Real v3 carrier shape: plain hosted pairs plus a cited text layout.
        let envelope = json!({"version":3, "source":"Anthropic:source", "response_id":"response",
        "blocks":[
            {"type":"server_tool_use", "id":"s", "name":"web_search", "input":{"query":"query"}},
            {"type":"web_search_tool_result", "tool_use_id":"s", "content":[{"type":"text", "text":"plain result"}]}
        ], "block_indices":[0,1], "layout":[
            {"kind":"pair", "index":0}, {"kind":"pair", "index":1},
            {"kind":"cited", "owner":"rigseg_response_0", "block":{"type":"text", "text":"answer", "citations":[citation]}}
        ]});
        let input: Vec<ResponseItem> = serde_json::from_value(json!([
            {"type":"web_search_call", "id":"search", "status":"completed", "action":{"type":"search", "query":"query"}, "wire_blocks":envelope},
            {"type":"message", "id":"rigseg_response_0", "role":"assistant", "content":[{"type":"output_text", "text":"answer"}]}
        ])).unwrap();
        let saved = annotated(&input, &selector);
        let before = saved.clone();
        let mut projected = input.clone();
        project_input(&mut projected, &sources_for_input(&saved), &selector);
        let mut expected = input.clone();
        if let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut expected[0] {
            *wire_blocks = None;
        }
        assert_eq!(projected, expected, "{field}");
        assert_eq!(saved, before);
        let mut account = selector;
        account.auth_domain = Some("account-v1:test-account".into());
        account.auth_domain_kind = Some("account".into());
        let mut projected = input.clone();
        project_input(
            &mut projected,
            &sources_for_input(&annotated(&input, &account)),
            &account,
        );
        assert_eq!(projected, input, "account-bound {field}");
    }
}

#[test]
fn selected_account_identity_partitions_users_workspaces_and_external_routing() {
    use base64::Engine;
    let auth = |user: &str, workspace: &str, revision: &str| {
        let claims =
            json!({"jti":revision, "https://api.openai.com/auth":{"chatgpt_user_id":user}});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        CodexAuth::from_external_chatgpt_tokens(
            &format!("header.{payload}.signature"),
            workspace,
            None,
        )
        .unwrap()
    };
    let mut info = provider();
    info.requires_openai_auth = true;
    let mut api = info.to_api_provider(None).unwrap();
    api.headers
        .insert("version", http::HeaderValue::from_static("test-version"));
    let captured = |api: &codex_api::Provider, auth: &CodexAuth| {
        let actual = codex_model_provider::auth_provider_from_auth(auth);
        request_source(
            &info,
            api,
            Some(auth),
            "model",
            actual.as_ref(),
            None,
            &http::HeaderMap::new(),
        )
        .unwrap()
    };
    let original = captured(&api, &auth("user-a", "workspace-a", "old"));
    assert_eq!(original.auth_domain_kind.as_deref(), Some("account"));
    assert_eq!(
        original,
        captured(&api, &auth("user-a", "workspace-a", "refreshed"))
    );
    assert_ne!(
        original.auth_domain,
        captured(&api, &auth("user-b", "workspace-a", "other-user")).auth_domain
    );
    assert_ne!(
        original.auth_domain,
        captured(&api, &auth("user-a", "workspace-b", "other-workspace")).auth_domain
    );
    let mut routed = api.clone();
    routed.headers.insert(
        http::header::COOKIE,
        http::HeaderValue::from_static("tenant=first"),
    );
    let first = captured(&routed, &auth("user-a", "workspace-a", "old"));
    assert_eq!(
        first.auth_domain_kind.as_deref(),
        Some("credentialInstance")
    );
    routed.headers.insert(
        http::header::COOKIE,
        http::HeaderValue::from_static("tenant=second"),
    );
    assert_ne!(
        first.auth_domain,
        captured(&routed, &auth("user-a", "workspace-a", "old")).auth_domain
    );
    routed.headers.remove(http::header::COOKIE);
    routed.query_params = Some([("api-key".into(), "first-private-query".into())].into());
    let first = captured(&routed, &auth("user-a", "workspace-a", "old"));
    routed.query_params = Some([("api-key".into(), "second-private-query".into())].into());
    assert_ne!(
        first.auth_domain,
        captured(&routed, &auth("user-a", "workspace-a", "old")).auth_domain
    );
}

#[test]
fn unrecognized_query_names_are_private_scope_not_account_or_anonymous() {
    let info = provider();
    let mut api = info.to_api_provider(None).unwrap();
    api.base_url = "https://endpoint.test/v1?sessionkey=first-secret".into();
    let scoped = request_source(
        &info,
        &api,
        None,
        "model-a",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(
        scoped.auth_domain_kind.as_deref(),
        Some("credentialInstance")
    );

    let ambient = CodexAuth::create_dummy_chatgpt_auth_for_testing();
    let account_attempt = request_source(
        &info,
        &api,
        Some(&ambient),
        "model-a",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_ne!(account_attempt.auth_domain_kind.as_deref(), Some("account"));

    let mut rotated = api;
    rotated.base_url = "https://endpoint.test/v1?sessionkey=rotated-secret".into();
    let rotated_source = request_source(
        &info,
        &rotated,
        None,
        "model-a",
        &StaticEmpty,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(scoped.endpoint_identity, rotated_source.endpoint_identity);
    assert_ne!(scoped.auth_domain, rotated_source.auth_domain);
    assert!(
        scoped
            .auth_domain
            .as_ref()
            .is_none_or(|domain| !domain.contains("secret"))
    );
}

#[test]
fn benign_extra_headers_do_not_rotate_identity_but_credential_headers_do() {
    let info = provider();
    let api = info.to_api_provider(None).unwrap();
    let bearer = StaticBearer {
        token: "Bearer stable",
    };
    let bare = request_source(
        &info,
        &api,
        None,
        "model",
        &bearer,
        None,
        &http::HeaderMap::new(),
    )
    .unwrap();
    assert_eq!(bare.auth_domain_kind.as_deref(), Some("credentialInstance"));

    let mut telemetry = http::HeaderMap::new();
    telemetry.insert(
        "x-codex-inference-call-id",
        http::HeaderValue::from_static("attempt-1"),
    );
    telemetry.insert("originator", http::HeaderValue::from_static("codex_vscode"));
    let with_telemetry =
        request_source(&info, &api, None, "model", &bearer, None, &telemetry).unwrap();
    assert_eq!(with_telemetry.auth_domain, bare.auth_domain);

    let mut vendor_first = http::HeaderMap::new();
    vendor_first.insert(
        "x-vendor-session",
        http::HeaderValue::from_static("session-first"),
    );
    let mut vendor_second = http::HeaderMap::new();
    vendor_second.insert(
        "x-vendor-session",
        http::HeaderValue::from_static("session-second"),
    );
    let first = request_source(&info, &api, None, "model", &bearer, None, &vendor_first).unwrap();
    let second = request_source(&info, &api, None, "model", &bearer, None, &vendor_second).unwrap();
    assert_eq!(
        first.auth_domain_kind.as_deref(),
        Some("credentialInstance")
    );
    assert_ne!(first.auth_domain, second.auth_domain);
    assert_ne!(first.auth_domain, bare.auth_domain);
    assert!(
        first
            .auth_domain
            .as_ref()
            .is_none_or(|domain| !domain.contains("session"))
    );
}

#[test]
fn header_only_credentials_preserve_same_scope_and_drop_opaque_after_rotation() {
    let info = provider();
    let api = info.to_api_provider(None).unwrap();
    for name in [
        "x-vendor-session",
        "x-codex-api-key",
        "x-openai-internal-auth",
        "x-b3-token",
    ] {
        for in_provider in [false, true] {
            let capture = |secret: &'static str| {
                let mut api = api.clone();
                let mut extra_headers = http::HeaderMap::new();
                let headers = if in_provider {
                    &mut api.headers
                } else {
                    &mut extra_headers
                };
                headers.insert(name, http::HeaderValue::from_static(secret));
                request_source(
                    &info,
                    &api,
                    /*auth*/ None,
                    "model-a",
                    &StaticEmpty,
                    /*agent_identity*/ None,
                    &extra_headers,
                )
                .unwrap()
            };
            let first = capture("first-private-key");
            let same = capture("first-private-key");
            let rotated = capture("rotated-private-key");
            assert_eq!(
                first.auth_domain_kind.as_deref(),
                Some("credentialInstance")
            );
            assert_eq!(same, first, "{name}, in_provider={in_provider}");
            assert_ne!(rotated.auth_domain, first.auth_domain);

            let items = fixture_items();
            let saved = annotated(&items, &first);
            let saved_before = saved.clone();
            let sources = sources_for_input(&saved);
            let mut projected = items.clone();
            project_input(&mut projected, &sources, &same);
            assert_eq!(projected, items, "stable {name}, in_provider={in_provider}");

            let mut expected = items;
            if let ResponseItem::Reasoning {
                encrypted_content, ..
            } = &mut expected[0]
            {
                *encrypted_content = None;
            }
            expected.remove(1);
            if let ResponseItem::WebSearchCall { wire_blocks, .. } = &mut expected[1] {
                *wire_blocks = None;
            }
            project_input(&mut projected, &sources, &rotated);
            assert_eq!(
                projected, expected,
                "rotated {name}, in_provider={in_provider}"
            );
            assert_eq!(saved, saved_before);
        }
    }
}
