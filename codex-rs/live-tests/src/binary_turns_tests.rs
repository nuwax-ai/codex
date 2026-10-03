//! Offline tests exercise the same runner, assertions and retention boundary
//! as marker, compaction and resumed hosted-search live scenes.

use super::*;
use pretty_assertions::assert_eq;
use runner::Completion;
use runner::ExecCapture;
use runner::ExecRunner;
use runner::ExecTurn;
use runner::PipeCapture;
use scenarios::BinaryScenario;
use scenarios::Scene;
use sha2::Digest;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

const ROLLOUT_NAME: &str = "rollout-file-2026-10-03T00-00-00.jsonl";
const MARKER: &str = "unforgeable-offline-marker";

fn rollout_path(home: &Path) -> PathBuf {
    home.join("sessions/2026/10/03").join(ROLLOUT_NAME)
}

fn write_rollout(home: &Path, bytes: &[u8]) {
    let path = rollout_path(home);
    std::fs::create_dir_all(path.parent().expect("rollout parent")).expect("session dir");
    std::fs::write(path, bytes).expect("rollout");
}

fn search_record(version: u64, ordinal: usize) -> Value {
    let call_id = format!("srv_{ordinal}");
    let response_id = format!("R{ordinal}");
    let cited = serde_json::json!({"type":"text", "text":format!("weather {ordinal}"),
        "citations":[{"type":"web_search_result_location", "url":"https://example.com/weather", "title":"Weather"}]});
    let mut envelope = serde_json::json!({"version":version,"source":"anthropic:test-source", "blocks":[
        {"type":"server_tool_use","id":call_id,"name":"web_search","input":{"query":"weather"}},
        {"type":"web_search_tool_result","tool_use_id":call_id,"content":[]}
    ]});
    if version == 1 {
        envelope["cited_text"] = serde_json::json!([cited]);
    } else {
        envelope["block_indices"] = serde_json::json!([0, 1]);
        envelope["layout"] = serde_json::json!([
            {"kind":"pair","index":0}, {"kind":"pair","index":1},
            {"kind":"cited","index":2,"owner":format!("rigseg_{response_id}_0"),"block":cited}
        ]);
        if version == 3 {
            envelope["response_id"] = serde_json::json!(response_id);
        }
    }
    serde_json::json!({"type":"response_item","payload":{"type":"web_search_call", "id":format!("ws_{ordinal}"),
        "status":"completed","wire_blocks":envelope}})
}

#[test]
fn retention_accepts_historical_v1_v2_and_current_identity_with_actual_counts() {
    let home = tempfile::tempdir().expect("home");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let records: Vec<_> = [1, 2, 3]
        .into_iter()
        .enumerate()
        .map(|(ordinal, version)| search_record(version, ordinal + 1).to_string())
        .collect();
    // Repeating a durable item does not count as another executed search.
    let bytes = format!("{}\n{}\n", records.join("\n"), records[2]);
    write_rollout(home.path(), bytes.as_bytes());
    assert_eq!(
        rollouts::retain_search_rollouts(home.path(), artifacts.path()).expect("valid versions"),
        rollouts::SearchEvidence {
            completed_pairs: 3,
            citations: 3
        }
    );
    home.close().expect("cleanup home");
    assert_eq!(
        std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("retained bytes"),
        bytes.into_bytes()
    );
}

#[test]
fn completed_status_alone_and_corrupt_identity_fail_after_retaining_bytes() {
    let valid = search_record(3, 1);
    let mut cases = Vec::new();
    for (field, value) in [
        ("version", serde_json::json!(99)),
        ("source", serde_json::json!("")),
        ("response_id", serde_json::json!("")),
        ("block_indices", serde_json::json!([0])),
        ("block_indices", serde_json::json!([0, 0])),
        ("block_indices", serde_json::json!([1, 0])),
    ] {
        let mut record = valid.clone();
        record["payload"]["wire_blocks"][field] = value;
        cases.push(record);
    }
    let mut wrong_owner = valid.clone();
    wrong_owner["payload"]["wire_blocks"]["layout"][2]["owner"] =
        serde_json::json!("rigseg_Another_0");
    cases.push(wrong_owner);
    let mut orphan_index = valid.clone();
    orphan_index["payload"]["wire_blocks"]["layout"][1]["index"] = serde_json::json!(9);
    orphan_index["payload"]["wire_blocks"]["layout"][2]["index"] = serde_json::json!(10);
    cases.push(orphan_index);
    let mut unmatched = valid.clone();
    unmatched["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] = serde_json::json!("other");
    cases.push(unmatched);
    let mut missing = valid;
    missing["payload"]
        .as_object_mut()
        .expect("payload")
        .remove("wire_blocks");
    cases.push(missing);
    for record in cases {
        let home = tempfile::tempdir().expect("home");
        let artifacts = tempfile::tempdir().expect("artifacts");
        let bytes = format!("{record}\n");
        write_rollout(home.path(), bytes.as_bytes());
        assert!(
            rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err(),
            "{record}"
        );
        home.close().expect("cleanup home");
        assert_eq!(
            std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("retained"),
            bytes.into_bytes()
        );
    }
}

#[test]
fn retention_preserves_truncated_non_utf8_bytes_before_parse_failure() {
    let home = tempfile::tempdir().expect("home");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let bytes = b"{\"type\":\"response_item\"\n\xff";
    write_rollout(home.path(), bytes);
    assert!(rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err());
    home.close().expect("cleanup home");
    assert_eq!(
        std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("retained"),
        bytes
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Launch,
    Exit,
    MissingFinal,
    MissingMarker,
    WrongPassphrase,
    NoCompaction,
    NoSearch,
    NoCitations,
    ReplaceBinary,
    PipeFailure,
    Timeout,
    WaitFailure,
    CaptureFailure,
}

#[derive(Clone, Copy)]
enum Kind {
    Marker,
    Compact,
    WebSearch,
}

struct FakeRunner {
    kind: Kind,
    fault: Option<(usize, Fault)>,
    turns: AtomicUsize,
    binaries: Mutex<Vec<PathBuf>>,
}

impl ExecRunner for FakeRunner {
    async fn run(&self, turn: &ExecTurn<'_>) -> Result<ExecCapture> {
        let ordinal = self.turns.fetch_add(1, Ordering::SeqCst) + 1;
        self.binaries
            .lock()
            .expect("binaries")
            .push(turn.binary.to_path_buf());
        let fault = self
            .fault
            .filter(|(step, _)| *step == ordinal)
            .map(|(_, fault)| fault);
        let path = rollout_path(turn.home);
        std::fs::create_dir_all(path.parent().expect("rollout parent"))?;
        let mut contents = std::fs::read_to_string(&path).unwrap_or_default();
        contents.push_str("{\"type\":\"session_meta\"}\n");
        if fault == Some(Fault::Launch) || fault == Some(Fault::Exit) {
            contents.push_str("{\"type\":\"response_item\"");
            std::fs::write(&path, &contents)?;
            if fault == Some(Fault::Launch) {
                anyhow::bail!("injected launch failure");
            }
        }
        let mut stdout = Vec::new();
        let answer = match self.kind {
            Kind::Marker => {
                stdout = serde_json::json!({"type":"item.completed","item":{
                    "type":"command_execution", "exit_code":0,
                    "aggregated_output":if fault == Some(Fault::MissingMarker) { "wrong output" } else { MARKER }
                }}).to_string().into_bytes();
                "done".to_string()
            }
            Kind::Compact => {
                if ordinal == 1 {
                    "OK".to_string()
                } else {
                    if fault != Some(Fault::NoCompaction) {
                        contents.push_str(
                            &serde_json::json!({"type":"compacted", "payload":{"message":MARKER}})
                                .to_string(),
                        );
                        contents.push('\n');
                    }
                    contents.push_str(&serde_json::json!({"type":"response_item", "payload":{
                        "type":"message", "role":"assistant", "content":[{"type":"output_text","text":MARKER}]}}).to_string());
                    contents.push('\n');
                    if fault == Some(Fault::WrongPassphrase) {
                        "forgotten".to_string()
                    } else {
                        MARKER.to_string()
                    }
                }
            }
            Kind::WebSearch => {
                let mut record = search_record(3, ordinal);
                if fault == Some(Fault::NoSearch) {
                    record["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] =
                        serde_json::json!("unmatched");
                }
                if fault == Some(Fault::NoCitations) {
                    record["payload"]["wire_blocks"]["layout"][2]["block"]["citations"] =
                        serde_json::json!([]);
                }
                contents.push_str(&record.to_string());
                contents.push('\n');
                format!("weather answer {ordinal}")
            }
        };
        std::fs::write(path, contents)?;
        if fault != Some(Fault::MissingFinal) {
            std::fs::write(turn.last_message, answer)?;
        }
        if fault == Some(Fault::ReplaceBinary) {
            std::fs::write(turn.binary, "replaced binary")?;
        }
        if fault == Some(Fault::Exit) {
            stdout.extend_from_slice(b"\xffpartial");
        }
        Ok(ExecCapture {
            stdout: PipeCapture {
                bytes: stdout,
                error: (fault == Some(Fault::PipeFailure))
                    .then(|| "injected pipe failure".to_string()),
            },
            stderr: PipeCapture {
                bytes: b"dispatch-log".to_vec(),
                error: None,
            },
            completion: match fault {
                Some(Fault::Timeout) => Completion::TimedOut,
                Some(Fault::WaitFailure) => {
                    Completion::WaitFailed("injected wait failure".to_string())
                }
                Some(Fault::CaptureFailure) => {
                    Completion::CaptureFailed("injected capture failure".to_string())
                }
                _ => Completion::Exited {
                    success: fault != Some(Fault::Exit),
                    description: if fault == Some(Fault::Exit) {
                        "injected exit failure"
                    } else {
                        "exit 0"
                    }
                    .to_string(),
                },
            },
        })
    }
}

struct Fixture {
    home: tempfile::TempDir,
    cwd: tempfile::TempDir,
    artifacts: tempfile::TempDir,
    prepared: PreparedExec,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("home");
        let cwd = tempfile::tempdir().expect("cwd");
        let artifacts = tempfile::tempdir().expect("artifacts");
        let path = cwd.path().join("prepared-exec");
        std::fs::write(&path, "prepared binary").expect("fixture executable");
        let sha256 = format!("{:x}", sha2::Sha256::digest(b"prepared binary"));
        Self {
            home,
            cwd,
            artifacts,
            prepared: PreparedExec { path, sha256 },
        }
    }

    async fn run(&self, kind: Kind, fault: Option<(usize, Fault)>) -> Result<()> {
        let runner = FakeRunner {
            kind,
            fault,
            turns: AtomicUsize::new(0),
            binaries: Mutex::new(Vec::new()),
        };
        let outcome = scenarios::run(
            &runner,
            &Scene {
                prepared: &self.prepared,
                home: self.home.path(),
                cwd: self.cwd.path(),
                artifacts: self.artifacts.path(),
                protocol: "offline",
                marker: MARKER,
            },
            match kind {
                Kind::Marker => BinaryScenario::Marker {
                    expect_bridge_log: Some("dispatch-log"),
                },
                Kind::Compact => BinaryScenario::Compact,
                Kind::WebSearch => BinaryScenario::WebSearch,
            },
        )
        .await;
        let binaries = runner.binaries.lock().expect("binary invocations");
        assert!(binaries.iter().all(|path| path == &self.prepared.path));
        outcome
    }
}

#[tokio::test]
async fn actual_scene_runner_matrix_retains_each_failure_after_home_cleanup() {
    for (kind, step, fault, expected) in [
        (Kind::Marker, 1, Fault::Launch, "injected launch failure"),
        (Kind::Marker, 1, Fault::Exit, "injected exit failure"),
        (
            Kind::Marker,
            1,
            Fault::MissingFinal,
            "produced no final answer file",
        ),
        (
            Kind::Marker,
            1,
            Fault::MissingMarker,
            "marker in its output",
        ),
        (Kind::Compact, 1, Fault::Launch, "injected launch failure"),
        (Kind::Compact, 2, Fault::Exit, "injected exit failure"),
        (
            Kind::Compact,
            2,
            Fault::MissingFinal,
            "produced no final answer file",
        ),
        (Kind::Compact, 2, Fault::WrongPassphrase, "passphrase lost"),
        (Kind::Compact, 2, Fault::NoCompaction, "compacted record"),
        (Kind::WebSearch, 1, Fault::Launch, "injected launch failure"),
        (Kind::WebSearch, 2, Fault::Exit, "injected exit failure"),
        (
            Kind::WebSearch,
            2,
            Fault::MissingFinal,
            "produced no final answer file",
        ),
        (Kind::WebSearch, 2, Fault::NoSearch, "matching call/result"),
        (
            Kind::Marker,
            1,
            Fault::ReplaceBinary,
            "changed after its receipt",
        ),
        (Kind::Marker, 1, Fault::PipeFailure, "injected pipe failure"),
        (Kind::Marker, 1, Fault::Timeout, "did not finish within"),
        (Kind::Marker, 1, Fault::WaitFailure, "injected wait failure"),
        (
            Kind::Marker,
            1,
            Fault::CaptureFailure,
            "output capture failed: injected capture failure",
        ),
    ] {
        let fixture = Fixture::new();
        let error = fixture
            .run(kind, Some((step, fault)))
            .await
            .expect_err("injected scene must fail");
        assert!(
            format!("{error:#}").contains(expected),
            "{fault:?}: {error:#}"
        );
        let original = std::fs::read(rollout_path(fixture.home.path())).expect("partial rollout");
        fixture.home.close().expect("cleanup temporary home");
        assert_eq!(
            std::fs::read(fixture.artifacts.path().join("rollout").join(ROLLOUT_NAME))
                .expect("retained"),
            original
        );
        if fault == Fault::Exit {
            let prefix = if step == 2 { "turn2." } else { "" };
            let events = std::fs::read(
                fixture
                    .artifacts
                    .path()
                    .join(format!("{prefix}events.jsonl")),
            )
            .expect("captured bytes");
            assert!(events.ends_with(b"\xffpartial"));
        }
    }
}

#[tokio::test]
async fn retention_and_capture_io_errors_preserve_the_original_scene_error() {
    let fixture = Fixture::new();
    std::fs::write(fixture.artifacts.path().join("rollout"), "conflict")
        .expect("retention conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::MissingMarker)))
        .await
        .expect_err("marker failure");
    assert_eq!(
        error.to_string(),
        "executed command must complete with exit 0 and the marker in its output"
    );
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.artifacts.path().join("events.jsonl")).expect("capture conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::Exit)))
        .await
        .expect_err("exit failure");
    assert_eq!(
        error.to_string(),
        "[offline]  exited with injected exit failure"
    );
    assert!(
        fixture
            .artifacts
            .path()
            .join("rollout")
            .join(ROLLOUT_NAME)
            .is_file()
    );
}

#[tokio::test]
async fn all_three_actual_scene_success_paths_use_prepared_binary() {
    for kind in [Kind::Marker, Kind::Compact, Kind::WebSearch] {
        Fixture::new()
            .run(kind, None)
            .await
            .expect("offline scene succeeds");
    }
}

#[tokio::test]
async fn changed_prepared_binary_fails_before_any_runner_call() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.prepared.path, "changed before turn").expect("replacement");
    let error = fixture
        .run(Kind::Marker, None)
        .await
        .expect_err("identity failure");
    assert!(error.to_string().contains("changed after its receipt"));
    assert!(!fixture.home.path().join("last_message.txt").exists());
    assert!(!rollout_path(fixture.home.path()).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn capture_pipe_bounds_descendant_drain_and_retains_partial_output() {
    let temp = tempfile::tempdir().expect("descendant directory");
    let finished = temp.path().join("finished");
    let mut child = tokio::process::Command::new("/bin/sh")
        .args([
            "-c",
            "printf partial; (sleep 2; : > \"$1\") &",
            "capture-helper",
        ])
        .arg(&finished)
        .stdout(Stdio::piped())
        .spawn()
        .expect("child");
    let pipe = child.stdout.take().expect("stdout");
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let reader = tokio::spawn(runner::capture_pipe(
        pipe,
        stopped,
        Duration::from_millis(30),
    ));
    assert!(child.wait().await.expect("exit").success());
    stop.send(true).expect("stop reader");
    let captured = tokio::time::timeout(Duration::from_secs(1), reader)
        .await
        .expect("bounded drain")
        .expect("reader");
    tokio::time::timeout(Duration::from_secs(4), async {
        while !finished.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("descendant exited");
    assert_eq!(
        (captured.bytes, captured.error.is_some()),
        (b"partial".to_vec(), true)
    );
}

#[cfg(unix)]
#[tokio::test]
async fn fake_executable_uses_real_process_capture_and_scene_retention() {
    use std::os::unix::fs::PermissionsExt;
    for exit_code in [0, 7] {
        let mut fixture = Fixture::new();
        let script = format!(
            r#"#!/bin/sh
last=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--output-last-message' ]; then
    shift
    last="$1"
  fi
  shift
done
mkdir -p "$CODEX_HOME/sessions/2026/10/03"
printf '%s\n' '{{"type":"session_meta"}}' > "$CODEX_HOME/sessions/2026/10/03/{ROLLOUT_NAME}"
printf '%s\n' 'done' > "$last"
printf '%s\n' '{{"type":"item.completed","item":{{"type":"command_execution","exit_code":0,"aggregated_output":"{MARKER}"}}}}'
printf '%s' 'dispatch-log' >&2
exit {exit_code}
"#
        );
        std::fs::write(&fixture.prepared.path, &script).expect("fake executable");
        std::fs::set_permissions(
            &fixture.prepared.path,
            std::fs::Permissions::from_mode(0o700),
        )
        .expect("executable permissions");
        fixture.prepared.sha256 = format!("{:x}", sha2::Sha256::digest(script.as_bytes()));
        let outcome = scenarios::run(
            &runner::ProcessRunner,
            &Scene {
                prepared: &fixture.prepared,
                home: fixture.home.path(),
                cwd: fixture.cwd.path(),
                artifacts: fixture.artifacts.path(),
                protocol: "subprocess",
                marker: MARKER,
            },
            BinaryScenario::Marker {
                expect_bridge_log: Some("dispatch-log"),
            },
        )
        .await;
        if exit_code == 0 {
            outcome.expect("real process success");
            assert!(fixture.artifacts.path().join("events.jsonl").is_file());
        } else {
            assert!(
                outcome
                    .expect_err("real process exit failure")
                    .to_string()
                    .contains("exited with")
            );
            fixture.home.close().expect("remove original home");
            assert_eq!(
                std::fs::read(fixture.artifacts.path().join("rollout").join(ROLLOUT_NAME))
                    .expect("retained bytes"),
                b"{\"type\":\"session_meta\"}\n"
            );
        }
    }
}

#[tokio::test]
async fn uncited_search_answer_records_zero_citations_and_keeps_basic_search_success() {
    let fixture = Fixture::new();
    fixture
        .run(Kind::WebSearch, Some((2, Fault::NoCitations)))
        .await
        .expect("legitimate uncited answer");
    let evidence: Value = serde_json::from_slice(
        &std::fs::read(fixture.artifacts.path().join("search-evidence.json")).expect("evidence"),
    )
    .expect("evidence JSON");
    assert_eq!(
        evidence,
        serde_json::json!({
            "turn1": {"answer_chars":16,"completed_matched_pairs":1,"citations":1},
            "turn2": {"answer_chars":16,"completed_matched_pairs":1,"citations":0},
            "citation_capability_asserted":false,
        })
    );
}

#[tokio::test]
async fn request_capture_presence_and_failures_are_recorded_without_masking_scene_failure() {
    let fixture = Fixture::new();
    fixture
        .run(Kind::Marker, None)
        .await
        .expect("uncaptured scene");
    let read_status = |fixture: &Fixture| -> Value {
        serde_json::from_slice(
            &std::fs::read(
                fixture
                    .artifacts
                    .path()
                    .join("request-capture-evidence.json"),
            )
            .expect("capture evidence"),
        )
        .expect("capture evidence JSON")
    };
    assert_eq!(
        read_status(&fixture),
        serde_json::json!({
            "read_status":"absent","http_attempts":0,"wire_asserted":false,"path":"requests.jsonl",
        })
    );
    let fixture = Fixture::new();
    std::fs::write(fixture.artifacts.path().join("requests.jsonl"),
        "{\"method\":\"POST\",\"url\":\"https://example.com/messages\",\"body_raw\":\"{}\",\"body\":{}}\n")
        .expect("trace");
    fixture
        .run(Kind::Marker, None)
        .await
        .expect("captured scene");
    assert_eq!(
        read_status(&fixture),
        serde_json::json!({
            "read_status":"available","http_attempts":1,"wire_asserted":false,"path":"requests.jsonl",
        })
    );
    let fixture = Fixture::new();
    std::fs::write(
        fixture.artifacts.path().join("requests.jsonl"),
        "truncated JSON",
    )
    .expect("malformed capture");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::MissingMarker)))
        .await
        .expect_err("scene failure");
    assert_eq!(
        error.to_string(),
        "executed command must complete with exit 0 and the marker in its output"
    );
    assert_eq!(
        read_status(&fixture)["read_status"],
        serde_json::json!("malformed")
    );
    assert!(
        fixture
            .artifacts
            .path()
            .join("rollout")
            .join(ROLLOUT_NAME)
            .is_file()
    );
}

#[test]
fn glm_prime_search_query_and_string_result_remain_verbatim_matched_evidence() {
    let home = tempfile::tempdir().expect("home");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let mut record = search_record(3, 1);
    let blocks = &mut record["payload"]["wire_blocks"]["blocks"];
    blocks[0]["name"] = serde_json::json!("web_search_prime");
    blocks[0]["input"] = serde_json::json!({
        "location":"cn", "search_query":"北京 今天 天气 2026年10月3日", "search_recency_filter":"oneDay",
    });
    blocks[1] = serde_json::json!({"type":"tool_result", "tool_use_id":"srv_1", "content":"gateway result"});
    let bytes = format!("{record}\n");
    write_rollout(home.path(), bytes.as_bytes());
    assert_eq!(
        rollouts::retain_search_rollouts(home.path(), artifacts.path())
            .expect("actual gateway shapes"),
        rollouts::SearchEvidence {
            completed_pairs: 1,
            citations: 1
        }
    );
    assert_eq!(
        std::fs::read(artifacts.path().join(ROLLOUT_NAME)).expect("verbatim retained bytes"),
        bytes.as_bytes()
    );
    // Accepting the gateway name cannot relax the call/result identity contract.
    record["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] =
        serde_json::json!("different-call");
    write_rollout(home.path(), format!("{record}\n").as_bytes());
    assert!(rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err());
    record["payload"]["wire_blocks"]["blocks"][1]["tool_use_id"] = serde_json::json!("srv_1");
    record["payload"]["wire_blocks"]["blocks"][0]["name"] = serde_json::json!("unrelated-tool");
    write_rollout(home.path(), format!("{record}\n").as_bytes());
    assert!(rollouts::retain_search_rollouts(home.path(), artifacts.path()).is_err());
}

#[tokio::test]
async fn post_execution_hash_failure_retains_received_bytes_and_wins_over_capture_io_error() {
    let fixture = Fixture::new();
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::ReplaceBinary)))
        .await
        .expect_err("hash must fail");
    assert!(error.to_string().contains("changed after its receipt"));
    let events: Value = serde_json::from_slice(
        &std::fs::read(fixture.artifacts.path().join("events.jsonl"))
            .expect("captured stdout before final verification"),
    )
    .expect("event JSON");
    assert_eq!(
        events,
        serde_json::json!({"type":"item.completed","item":{
            "type":"command_execution", "exit_code":0, "aggregated_output":MARKER,
        }})
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log")).expect("stderr retained"),
        b"dispatch-log"
    );
    let exit = std::fs::read_to_string(fixture.artifacts.path().join("exit.txt"))
        .expect("exit diagnostic");
    assert!(exit.starts_with("exit 0\n"));
    assert!(exit.contains("executable identity check failed"));
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.artifacts.path().join("events.jsonl"))
        .expect("capture I/O conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::ReplaceBinary)))
        .await
        .expect_err("hash remains primary");
    assert!(error.to_string().contains("changed after its receipt"));
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log"))
            .expect("remaining captures still written"),
        b"dispatch-log"
    );
}

#[tokio::test]
async fn capture_pipe_keeps_exact_limit_and_reports_overflow_with_the_complete_allowed_prefix() {
    let (_stop, stopped) = tokio::sync::watch::channel(/*init*/ false);
    let exact = vec![b'x'; runner::MAX_PIPE_BYTES];
    let capture = runner::capture_pipe(
        exact.as_slice(),
        stopped.clone(),
        Duration::from_secs(/*secs*/ 1),
    )
    .await;
    assert_eq!((capture.bytes, capture.error), (exact.clone(), None));
    let mut oversized = exact.clone();
    oversized.extend_from_slice(b"overflow");
    let capture = runner::capture_pipe(
        oversized.as_slice(),
        stopped,
        Duration::from_secs(/*secs*/ 1),
    )
    .await;
    assert_eq!(
        (capture.bytes, capture.error),
        (
            exact,
            Some("pipe exceeded 16 MiB capture limit".to_string())
        )
    );
}

#[tokio::test]
async fn capture_failure_remains_primary_when_artifact_io_also_fails() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.artifacts.path().join("events.jsonl"))
        .expect("capture I/O conflict");
    let error = fixture
        .run(Kind::Marker, Some((1, Fault::CaptureFailure)))
        .await
        .expect_err("capture failure");
    assert_eq!(
        error.to_string(),
        "[offline]  output capture failed: injected capture failure"
    );
    assert!(fixture.artifacts.path().join("stderr.log").is_file());
    assert!(
        fixture
            .artifacts
            .path()
            .join("rollout")
            .join(ROLLOUT_NAME)
            .is_file()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn real_process_output_overflow_stops_the_child_before_its_wait_deadline_and_retains_both_pipes()
 {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let size = runner::MAX_PIPE_BYTES + 1;
    let script = format!(
        "#!/bin/sh\nprintf '%s' 'stderr prefix' >&2\nhead -c {size} /dev/zero\nexec sleep 60\n"
    );
    std::fs::write(&fixture.prepared.path, &script).expect("fake executable");
    std::fs::set_permissions(
        &fixture.prepared.path,
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("executable");
    fixture.prepared.sha256 = format!("{:x}", sha2::Sha256::digest(script.as_bytes()));
    let outcome = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 10),
        scenarios::run(
            &runner::ProcessRunner,
            &Scene {
                prepared: &fixture.prepared,
                home: fixture.home.path(),
                cwd: fixture.cwd.path(),
                artifacts: fixture.artifacts.path(),
                protocol: "overflow",
                marker: MARKER,
            },
            BinaryScenario::Marker {
                expect_bridge_log: None,
            },
        ),
    )
    .await
    .expect("capture overflow must stop the child promptly");
    let error = outcome.expect_err("explicit capture failure");
    assert!(
        error
            .to_string()
            .contains("output capture failed: stdout: pipe exceeded 16 MiB capture limit")
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("events.jsonl"))
            .expect("retained stdout prefix"),
        vec![0; runner::MAX_PIPE_BYTES]
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log")).expect("drained stderr"),
        b"stderr prefix"
    );
    let exit =
        std::fs::read_to_string(fixture.artifacts.path().join("exit.txt")).expect("exit evidence");
    assert!(exit.starts_with("output capture failed:"));
    assert!(!exit.contains("timeout after"));
}

#[tokio::test]
async fn capture_pipe_read_failure_preserves_every_preceding_completed_read() {
    struct FailsAfterPrefix {
        prefix: Option<&'static [u8]>,
    }
    impl tokio::io::AsyncRead for FailsAfterPrefix {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            match self.get_mut().prefix.take() {
                Some(prefix) => {
                    buffer.put_slice(prefix);
                    std::task::Poll::Ready(Ok(()))
                }
                None => {
                    std::task::Poll::Ready(Err(std::io::Error::other("injected pipe read failure")))
                }
            }
        }
    }
    let (_stop, stopped) = tokio::sync::watch::channel(/*init*/ false);
    let capture = runner::capture_pipe(
        FailsAfterPrefix {
            prefix: Some(b"complete prefix"),
        },
        stopped,
        Duration::from_secs(/*secs*/ 1),
    )
    .await;
    assert_eq!(
        (capture.bytes, capture.error),
        (
            b"complete prefix".to_vec(),
            Some("injected pipe read failure".to_string())
        )
    );
}

#[cfg(unix)]
#[tokio::test]
async fn known_nonzero_child_exit_remains_primary_over_later_pipe_drain_failure() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let script = "#!/bin/sh\nprintf '%s' 'stdout prefix'\nprintf '%s' 'stderr prefix' >&2\n(sleep 6; : > \"$CODEX_HOME/descendant-finished\") &\nexit 7\n";
    std::fs::write(&fixture.prepared.path, script).expect("fake executable");
    std::fs::set_permissions(
        &fixture.prepared.path,
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("executable");
    fixture.prepared.sha256 = format!("{:x}", sha2::Sha256::digest(script.as_bytes()));
    let outcome = scenarios::run(
        &runner::ProcessRunner,
        &Scene {
            prepared: &fixture.prepared,
            home: fixture.home.path(),
            cwd: fixture.cwd.path(),
            artifacts: fixture.artifacts.path(),
            protocol: "late-drain",
            marker: MARKER,
        },
        BinaryScenario::Marker {
            expect_bridge_log: None,
        },
    )
    .await;
    tokio::time::timeout(Duration::from_secs(/*secs*/ 5), async {
        while !fixture.home.path().join("descendant-finished").exists() {
            tokio::time::sleep(Duration::from_millis(/*millis*/ 20)).await;
        }
    })
    .await
    .expect("owned descendant teardown");
    let error = outcome.expect_err("natural exit must fail");
    assert!(error.to_string().starts_with("[late-drain]  exited with"));
    let exit =
        std::fs::read_to_string(fixture.artifacts.path().join("exit.txt")).expect("exit evidence");
    assert!(exit.contains("capture incomplete: pipe did not close"));
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("events.jsonl")).expect("stdout prefix"),
        b"stdout prefix"
    );
    assert_eq!(
        std::fs::read(fixture.artifacts.path().join("stderr.log")).expect("stderr prefix"),
        b"stderr prefix"
    );
}
