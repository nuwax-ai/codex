//! Offline tests exercise the same runner, assertions and retention boundary
//! as marker, compaction and resumed hosted-search live scenes.

use super::super::*;
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

pub(crate) const ROLLOUT_NAME: &str = "rollout-file-2026-10-03T00-00-00.jsonl";
pub(crate) const MARKER: &str = "unforgeable-offline-marker";

pub(crate) fn rollout_path(home: &Path) -> PathBuf {
    home.join("sessions/2026/10/03").join(ROLLOUT_NAME)
}

pub(crate) fn write_rollout(home: &Path, bytes: &[u8]) {
    let path = rollout_path(home);
    std::fs::create_dir_all(path.parent().expect("rollout parent")).expect("session dir");
    std::fs::write(path, bytes).expect("rollout");
}

pub(crate) fn search_record(version: u64, ordinal: usize) -> Value {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
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
pub(crate) enum Kind {
    Marker,
    Compact,
    WebSearch,
}

pub(crate) struct FakeRunner {
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

pub(crate) struct Fixture {
    pub(crate) home: tempfile::TempDir,
    pub(crate) cwd: tempfile::TempDir,
    pub(crate) artifacts: tempfile::TempDir,
    pub(crate) prepared: PreparedExec,
}

impl Fixture {
    pub(crate) fn new() -> Self {
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

    pub(crate) async fn run(&self, kind: Kind, fault: Option<(usize, Fault)>) -> Result<()> {
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
                expected_model: "test-model",
                expected_url_prefix: "https://unit.test/v1",
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
