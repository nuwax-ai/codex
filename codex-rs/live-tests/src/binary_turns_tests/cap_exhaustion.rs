//! The expected product failure still requires complete, durable evidence.

use super::super::*;
use super::common::*;
use capture_validation::CapExpectation;
use capture_validation::CaptureRequirement;
use codex_rust_rig_bridge::RigProtocol;
use pretty_assertions::assert_eq;
use runner::Completion;
use runner::ExecCapture;
use runner::ExecRunner;
use runner::ExecTurn;

const CAP_TERMINAL: &str = "Output token limit reached; increase max_tokens before retrying";
const PARTIAL_ROLLOUT: &[u8] = b"{\"type\":\"session_meta\"}\n{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"partial lighthouse story\"}]}}\n\xffpartial";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CapCase {
    Exhausted,
    SuccessfulExit,
    StdoutPipeFailure,
    StderrPipeFailure,
    MalformedEvent,
    InvalidUtf8,
    ExtraAttempt,
    MissingAttempt,
    Completed,
    WrongTerminal,
    WrongStderr,
}

struct CapRunner {
    base: FakeRunner,
    case: CapCase,
    wire: RigProtocol,
}

impl ExecRunner for CapRunner {
    async fn run(&self, turn: &ExecTurn<'_>) -> Result<ExecCapture> {
        let mut capture = self.base.run(turn).await?;
        write_rollout(turn.home, PARTIAL_ROLLOUT);
        std::fs::remove_file(turn.last_message)?;
        let message = if self.case == CapCase::WrongTerminal {
            "gateway unavailable"
        } else {
            CAP_TERMINAL
        };
        capture.stdout.bytes = format!(
            "\n{}\n \t\n",
            serde_json::json!({"type":"turn.failed","error":{"message":message}})
        )
        .into_bytes();
        capture.stderr.bytes = if self.case == CapCase::WrongStderr {
            b"gateway unavailable".to_vec()
        } else {
            CAP_TERMINAL.as_bytes().to_vec()
        };
        capture.completion = Completion::Exited {
            success: self.case == CapCase::SuccessfulExit,
            description: "fixture cap exit".to_string(),
        };
        match self.case {
            CapCase::StdoutPipeFailure => {
                capture.stdout.error = Some("injected stdout drain failure".to_string());
            }
            CapCase::StderrPipeFailure => {
                capture.stderr.error = Some("injected stderr drain failure".to_string());
            }
            CapCase::MalformedEvent => capture.stdout.bytes.extend_from_slice(b"{broken\n"),
            CapCase::InvalidUtf8 => capture.stdout.bytes.extend_from_slice(b"\xff\n"),
            CapCase::Completed => capture
                .stdout
                .bytes
                .extend_from_slice(b"{\"type\":\"turn.completed\"}\n"),
            CapCase::Exhausted
            | CapCase::SuccessfulExit
            | CapCase::ExtraAttempt
            | CapCase::MissingAttempt
            | CapCase::WrongTerminal
            | CapCase::WrongStderr => {}
        }
        if self.case != CapCase::MissingAttempt {
            let (path, cap_field) = match self.wire {
                RigProtocol::Responses => ("responses", "max_output_tokens"),
                RigProtocol::Chat => ("chat/completions", "max_tokens"),
                RigProtocol::Anthropic => ("messages", "max_tokens"),
            };
            let mut body = serde_json::json!({"model":"test-model"});
            body[cap_field] = serde_json::json!(32);
            let attempt = serde_json::json!({
                "url":format!("https://unit.test/v1/{path}"),"body":body,
            });
            let count = if self.case == CapCase::ExtraAttempt {
                2
            } else {
                1
            };
            std::fs::write(turn.request_capture, format!("{attempt}\n").repeat(count))?;
        }
        Ok(capture)
    }
}

async fn run_cap(fixture: &Fixture, case: CapCase, wire: RigProtocol) -> Result<()> {
    scenarios::run(
        &CapRunner {
            base: FakeRunner::new(Kind::Marker),
            case,
            wire,
        },
        &Scene {
            prepared: &fixture.prepared,
            home: fixture.home.path(),
            cwd: fixture.cwd.path(),
            artifacts: fixture.artifacts.path(),
            protocol: "offline-cap",
            marker: MARKER,
            expected_model: "test-model",
            expected_url_prefix: "https://unit.test/v1",
            wire,
            capture_requirement: CaptureRequirement::Required,
            expected_cap: CapExpectation::Explicit(32),
        },
        BinaryScenario::CapExhausted,
    )
    .await
}

#[tokio::test]
async fn cap_scene_accepts_one_failed_attempt_and_retains_partial_rollout() {
    for (wire, cap_field) in [
        (RigProtocol::Chat, "body.max_tokens"),
        (RigProtocol::Anthropic, "body.max_tokens"),
        (RigProtocol::Responses, "body.max_output_tokens"),
    ] {
        let fixture = Fixture::new();
        run_cap(&fixture, CapCase::Exhausted, wire)
            .await
            .expect("expected cap failure is valid evidence");
        let evidence: Value = serde_json::from_slice(
            &std::fs::read(
                fixture
                    .artifacts
                    .path()
                    .join("request-capture-evidence.json"),
            )
            .expect("request evidence"),
        )
        .expect("request evidence JSON");
        assert_eq!(
            evidence,
            serde_json::json!({
                "read_status":"available","http_attempts":1,"wire_asserted":true,
                "asserted_fields":["url.scheme","url.authority","url.path","body.model",cap_field],
                "path":"requests.jsonl",
            })
        );
        fixture.home.close().expect("remove temporary home");
        assert_eq!(
            std::fs::read(fixture.artifacts.path().join("rollout").join(ROLLOUT_NAME))
                .expect("durable partial rollout"),
            PARTIAL_ROLLOUT
        );
    }
}

#[tokio::test]
async fn cap_scene_rejects_incomplete_or_contradictory_terminal_evidence() {
    for (case, expected) in [
        (CapCase::SuccessfulExit, "completed successfully"),
        (CapCase::StdoutPipeFailure, "injected stdout drain failure"),
        (CapCase::StderrPipeFailure, "injected stderr drain failure"),
        (CapCase::MalformedEvent, "parse capped-out turn event"),
        (CapCase::InvalidUtf8, "read capped-out turn events as UTF-8"),
        (CapCase::ExtraAttempt, "exactly one HTTP attempt"),
        (
            CapCase::MissingAttempt,
            "required Rig request capture is absent",
        ),
        (CapCase::Completed, "must not report a completed turn"),
        (
            CapCase::WrongTerminal,
            "must fail with the cap terminal event",
        ),
        (
            CapCase::WrongStderr,
            "must surface the product's cap terminal on stderr",
        ),
    ] {
        let fixture = Fixture::new();
        let error = run_cap(&fixture, case, RigProtocol::Chat)
            .await
            .expect_err("invalid cap evidence must fail");
        assert!(
            format!("{error:#}").contains(expected),
            "{case:?}: {error:#}"
        );
        let artifacts = fixture.artifacts.path();
        assert!(artifacts.join("events.jsonl").is_file());
        assert!(artifacts.join("stderr.log").is_file());
        assert!(artifacts.join("exit.txt").is_file());
        let evidence: Value = serde_json::from_slice(
            &std::fs::read(artifacts.join("request-capture-evidence.json"))
                .expect("rejected request evidence"),
        )
        .expect("rejected request evidence JSON");
        let expected_evidence = if case == CapCase::MissingAttempt {
            serde_json::json!({
                "read_status":"absent","http_attempts":0,"wire_asserted":false,
                "asserted_fields":[],"path":"requests.jsonl",
            })
        } else {
            serde_json::json!({
                "read_status":"available",
                "http_attempts":if case == CapCase::ExtraAttempt { 2 } else { 1 },
                "wire_asserted":true,
                "asserted_fields":["url.scheme","url.authority","url.path","body.model","body.max_tokens"],
                "path":"requests.jsonl",
            })
        };
        assert_eq!(evidence, expected_evidence);
        fixture.home.close().expect("remove temporary home");
        assert_eq!(
            std::fs::read(artifacts.join("rollout").join(ROLLOUT_NAME))
                .expect("retained rejected cap rollout"),
            PARTIAL_ROLLOUT
        );
    }
}

#[tokio::test]
async fn cap_scene_cannot_pass_without_retaining_its_rollout() {
    let fixture = Fixture::new();
    std::fs::write(fixture.artifacts.path().join("rollout"), "conflict")
        .expect("retention conflict");
    let error = run_cap(&fixture, CapCase::Exhausted, RigProtocol::Chat)
        .await
        .expect_err("accepted terminal requires durable rollout");
    assert_eq!(error.to_string(), "retain capped-out turn rollout");
    let error = run_cap(&fixture, CapCase::WrongTerminal, RigProtocol::Chat)
        .await
        .expect_err("retention conflict cannot replace terminal assertion");
    assert!(
        error
            .to_string()
            .contains("must fail with the cap terminal event")
    );
}
