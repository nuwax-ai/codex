use super::super::*;
use super::common::*;
use capture_validation::CaptureRequirement;
use codex_rust_rig_bridge::RigProtocol;
use pretty_assertions::assert_eq;

#[test]
fn captured_endpoint_checks_authority_protocol_path_and_model_without_url_secrets() {
    for (wire, suffix) in [
        (RigProtocol::Responses, "responses"),
        (RigProtocol::Chat, "chat/completions"),
        // A base already carrying the version segment gets only /messages.
        (RigProtocol::Anthropic, "messages"),
    ] {
        let expected = "https://user:private-password@UNIT.test:443/v1?token=private-token";
        let valid = serde_json::json!({"url": format!("https://unit.test/v1/{suffix}?token=REDACTED"),"body":{"model":"test-model"}});
        capture_validation::validate_attempt(&valid, expected, wire, "test-model")
            .expect("normalized redacted route");
        for url in [
            format!("https://unit.test.evil/v1/{suffix}"),
            format!("http://unit.test/v1/{suffix}"),
            format!("https://unit.test:444/v1/{suffix}"),
            "https://unit.test/v11/responses".to_string(),
            "https://unit.test/v1/wrong".to_string(),
        ] {
            let invalid = serde_json::json!({"url":url,"body":{"model":"test-model"}});
            let error =
                capture_validation::validate_attempt(&invalid, expected, wire, "test-model")
                    .expect_err("wrong endpoint");
            assert!(!format!("{error:#}").contains("private-password"));
            assert!(!format!("{error:#}").contains("private-token"));
        }
        let wrong_model = serde_json::json!({"url":valid["url"],"body":{"model":"wrong-model"}});
        assert!(
            capture_validation::validate_attempt(&wrong_model, expected, wire, "test-model")
                .is_err()
        );
    }
}

#[test]
fn anthropic_capture_paths_match_all_sdk_base_suffixes_and_retain_gateway_prefixes() {
    let valid = serde_json::json!({
        "url": "https://unit.test/gateway/v1/messages?token=REDACTED",
        "body": {"model":"test-model"},
    });
    for base_path in [
        "/gateway",
        "/gateway/v1",
        "/gateway/messages",
        "/gateway/v1/messages",
        "/gateway/v1/messages/",
    ] {
        let base =
            format!("https://user:private-password@UNIT.test:443{base_path}?token=private-token");
        capture_validation::validate_attempt(&valid, &base, RigProtocol::Anthropic, "test-model")
            .expect("SDK-normalized path");
        let wrong_prefix = serde_json::json!({
            "url": "https://unit.test/v1/messages",
            "body": {"model":"test-model"},
        });
        let error = capture_validation::validate_attempt(
            &wrong_prefix,
            &base,
            RigProtocol::Anthropic,
            "test-model",
        )
        .expect_err("gateway namespace cannot disappear");
        assert!(!format!("{error:#}").contains("private-password"));
        assert!(!format!("{error:#}").contains("private-token"));
    }
}

#[test]
fn raw_capture_validation_keeps_huge_schema_numbers_and_uses_authoritative_fields() {
    use capture_validation::CapExpectation;

    let raw = r#"{ "model" : "test-model", "max_output_tokens" : 512, "tools" : [{"parameters":{"limit":1e999,"count":18446744073709551617}}] }"#;
    let attempt = serde_json::json!({
        "url": "https://unit.test/v1/responses",
        "body_raw": raw,
        "body": null,
    });
    capture_validation::validate_attempt(
        &attempt,
        "https://unit.test/v1",
        RigProtocol::Responses,
        "test-model",
    )
    .expect("unrepresentable convenience body cannot invalidate raw fields");
    assert_eq!(
        capture_validation::validate_cap(
            &attempt,
            RigProtocol::Responses,
            CapExpectation::Explicit(512),
        )
        .expect("raw cap"),
        "body.max_output_tokens"
    );
    assert_eq!(attempt["body_raw"].as_str(), Some(raw));

    for (raw, wrong_model) in [
        (r#"{"model":"wrong-model","max_output_tokens":512}"#, true),
        (r#"{"model":"test-model","max_output_tokens":1024}"#, false),
    ] {
        let attempt = serde_json::json!({
            "url":"https://unit.test/v1/responses",
            "body_raw":raw,
            "body":{"model":"test-model","max_output_tokens":512},
        });
        let result = if wrong_model {
            capture_validation::validate_attempt(
                &attempt,
                "https://unit.test/v1",
                RigProtocol::Responses,
                "test-model",
            )
        } else {
            capture_validation::validate_cap(
                &attempt,
                RigProtocol::Responses,
                CapExpectation::Explicit(512),
            )
            .map(|_| ())
        };
        assert!(
            result.is_err(),
            "convenience fields cannot override raw wire"
        );
    }
}

#[test]
fn malformed_raw_capture_never_falls_back_to_correct_convenience_fields() {
    use capture_validation::CapExpectation;

    for raw in [
        serde_json::json!("{broken"),
        serde_json::json!("null"),
        serde_json::json!("[]"),
        serde_json::Value::Null,
    ] {
        let attempt = serde_json::json!({
            "url":"https://unit.test/v1/responses",
            "body_raw":raw,
            "body":{"model":"test-model","max_output_tokens":512},
        });
        assert!(
            capture_validation::validate_attempt(
                &attempt,
                "https://unit.test/v1",
                RigProtocol::Responses,
                "test-model",
            )
            .is_err()
        );
        assert!(
            capture_validation::validate_cap(
                &attempt,
                RigProtocol::Responses,
                CapExpectation::Explicit(512),
            )
            .is_err()
        );
    }
}

#[test]
fn raw_cap_validation_retains_presence_and_exact_integer_semantics() {
    use capture_validation::CapExpectation;

    for (raw, wire, expectation, field) in [
        (
            r#"{"max_output_tokens":512}"#,
            RigProtocol::Responses,
            CapExpectation::Explicit(512),
            "body.max_output_tokens",
        ),
        (
            r#"{"max_tokens":512}"#,
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
            "body.max_tokens",
        ),
        (
            r#"{"max_completion_tokens":512}"#,
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
            "body.max_completion_tokens",
        ),
        (
            r#"{"max_tokens":512}"#,
            RigProtocol::Anthropic,
            CapExpectation::Explicit(512),
            "body.max_tokens",
        ),
        (
            r#"{"max_tokens":16384}"#,
            RigProtocol::Anthropic,
            CapExpectation::AnthropicDefault,
            "body.max_tokens",
        ),
        (
            "{}",
            RigProtocol::Chat,
            CapExpectation::Absent,
            "body.max_tokens_absent",
        ),
        (
            "{}",
            RigProtocol::Responses,
            CapExpectation::Absent,
            "body.max_output_tokens_absent",
        ),
    ] {
        let attempt = serde_json::json!({"body_raw":raw,"body":null});
        assert_eq!(
            capture_validation::validate_cap(&attempt, wire, expectation)
                .expect("raw protocol cap"),
            field
        );
    }

    for (raw, wire, expectation) in [
        (
            r#"{"max_output_tokens":null}"#,
            RigProtocol::Responses,
            CapExpectation::Absent,
        ),
        (
            r#"{"max_tokens":null}"#,
            RigProtocol::Chat,
            CapExpectation::Absent,
        ),
        (
            r#"{"max_tokens":512,"max_completion_tokens":null}"#,
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
        ),
        (
            r#"{"max_tokens":512.0}"#,
            RigProtocol::Anthropic,
            CapExpectation::Explicit(512),
        ),
        (
            r#"{"max_output_tokens":18446744073709551617}"#,
            RigProtocol::Responses,
            CapExpectation::Explicit(512),
        ),
    ] {
        let attempt = serde_json::json!({"body_raw":raw,"body":{}});
        assert!(capture_validation::validate_cap(&attempt, wire, expectation).is_err());
    }
}

#[tokio::test]
async fn capture_scene_requires_real_attempts_before_asserting_wire_evidence() {
    for contents in [
        None,
        Some(""),
        Some(" \n\t\n"),
        Some("{broken"),
        Some("{\"url\":\"https://unit.test/v1/responses\",\"body\":{\"model\":\"test-model\"}}\n"),
    ] {
        let fixture = Fixture::new();
        if let Some(contents) = contents {
            std::fs::write(fixture.artifacts.path().join("requests.jsonl"), contents)
                .expect("capture");
        }
        // The runner exercises the normal scene/retention path; its existing
        // fake binary is intentionally allowed to omit native capture.
        let runner = FakeRunner::new(Kind::Marker);
        let result = scenarios::run(
            &runner,
            &Scene {
                prepared: &fixture.prepared,
                home: fixture.home.path(),
                cwd: fixture.cwd.path(),
                artifacts: fixture.artifacts.path(),
                protocol: "offline",
                marker: MARKER,
                expected_model: "test-model",
                expected_url_prefix: "https://unit.test/v1",
                wire: RigProtocol::Responses,
                capture_requirement: CaptureRequirement::Required,
                expected_cap: capture_validation::CapExpectation::Absent,
            },
            BinaryScenario::Marker {
                expect_bridge_log: Some("dispatch-log"),
            },
        )
        .await;
        let valid = contents.is_some_and(|contents| contents.starts_with("{\"url\""));
        assert_eq!(result.is_ok(), valid);
        let evidence: Value = serde_json::from_slice(
            &std::fs::read(
                fixture
                    .artifacts
                    .path()
                    .join("request-capture-evidence.json"),
            )
            .expect("evidence"),
        )
        .expect("JSON");
        assert_eq!(evidence["wire_asserted"], serde_json::json!(valid));
        assert_eq!(
            evidence["http_attempts"],
            serde_json::json!(usize::from(
                contents.is_some_and(|contents| !contents.trim().is_empty())
            ))
        );
        if !valid {
            assert_eq!(evidence["asserted_fields"], serde_json::json!([]));
            assert!(
                fixture
                    .artifacts
                    .path()
                    .join("rollout")
                    .join(ROLLOUT_NAME)
                    .exists()
            );
        }
    }
}

#[test]
fn cap_expectations_pin_the_typed_field_per_protocol() {
    use capture_validation::CapExpectation;

    let attempt = |body: serde_json::Value| serde_json::json!({"url": "https://unit.test/v1/responses", "body": body});
    // Explicit caps reach the protocol's own field.
    assert_eq!(
        capture_validation::validate_cap(
            &attempt(serde_json::json!({"max_output_tokens": 512})),
            RigProtocol::Responses,
            CapExpectation::Explicit(512),
        )
        .unwrap(),
        "body.max_output_tokens"
    );
    assert_eq!(
        capture_validation::validate_cap(
            &attempt(serde_json::json!({"max_tokens": 512})),
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
        )
        .unwrap(),
        "body.max_tokens"
    );
    // Reasoning-model endpoints reject the legacy field; the modern spelling
    // is equally valid evidence.
    assert_eq!(
        capture_validation::validate_cap(
            &attempt(serde_json::json!({"max_completion_tokens": 512})),
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
        )
        .unwrap(),
        "body.max_completion_tokens"
    );
    assert_eq!(
        capture_validation::validate_cap(
            &attempt(serde_json::json!({"max_tokens": 512})),
            RigProtocol::Anthropic,
            CapExpectation::Explicit(512),
        )
        .unwrap(),
        "body.max_tokens"
    );
    assert_eq!(
        capture_validation::validate_cap(
            &attempt(serde_json::json!({"max_tokens": 16384})),
            RigProtocol::Anthropic,
            CapExpectation::AnthropicDefault,
        )
        .unwrap(),
        "body.max_tokens"
    );
    assert_eq!(
        capture_validation::validate_cap(
            &attempt(serde_json::json!({})),
            RigProtocol::Chat,
            CapExpectation::Absent,
        )
        .unwrap(),
        "body.max_tokens_absent"
    );

    // Wrong value, invented fields, missing required fields, dual-field
    // conflicts and impossible combinations all fail loudly.
    for (body, wire, expectation) in [
        (
            serde_json::json!({"max_output_tokens": 1024}),
            RigProtocol::Responses,
            CapExpectation::Explicit(512),
        ),
        (
            serde_json::json!({"max_tokens": 1024}),
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
        ),
        // No cap configured: the wire must not invent one.
        (
            serde_json::json!({"max_tokens": 4096}),
            RigProtocol::Chat,
            CapExpectation::Absent,
        ),
        (
            serde_json::json!({"max_output_tokens": 4096}),
            RigProtocol::Responses,
            CapExpectation::Absent,
        ),
        // Anthropic requires the field with the bridge default.
        (
            serde_json::json!({}),
            RigProtocol::Anthropic,
            CapExpectation::AnthropicDefault,
        ),
        (
            serde_json::json!({"max_tokens": 4096}),
            RigProtocol::Anthropic,
            CapExpectation::AnthropicDefault,
        ),
        // Chat cannot carry both spellings.
        (
            serde_json::json!({"max_tokens": 512, "max_completion_tokens": 512}),
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
        ),
        // Chat explicit with no field at all.
        (
            serde_json::json!({}),
            RigProtocol::Chat,
            CapExpectation::Explicit(512),
        ),
        // Absent is not a legal Anthropic expectation.
        (
            serde_json::json!({}),
            RigProtocol::Anthropic,
            CapExpectation::Absent,
        ),
    ] {
        let rejected = serde_json::to_string(&body).unwrap_or_default();
        let result = capture_validation::validate_cap(&attempt(body), wire, expectation);
        assert!(result.is_err(), "must reject {rejected} on {wire:?}");
    }
}
