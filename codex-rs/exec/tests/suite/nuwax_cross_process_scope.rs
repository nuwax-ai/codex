//! S1 cross-process conservative replay at the binary level: a REAL `codex exec`
//! turn produces history containing opaque reasoning ciphertext; a second,
//! isolated process resumes a COPY of that rollout with rotated endpoint
//! query shapes and credentials. Visible history must replay on the wire,
//! while opaque ciphertext degrades because credential identities are
//! process-local, including when the second process uses the same key.
//! Credential/query rotation branches need separate same-process controls.
//! The copied rollout's existing bytes are never rewritten by the resume.

use anyhow::Context;
use anyhow::Result;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

const CONFIG_ENV_VARIABLES: [&str; 12] = [
    "NUWAX_BASE_URL",
    "NUWAX_WIRE_API",
    "NUWAX_API_KEY",
    "NUWAX_MODEL",
    "NUWAX_MAX_OUTPUT_TOKENS",
    "NUWAX_REQUEST_MAX_RETRIES",
    "NUWAX_STREAM_MAX_RETRIES",
    "NUWAX_STREAM_IDLE_TIMEOUT_MS",
    "CODEX_MODEL_REASONING_EFFORT",
    "CODEX_MODEL_CONTEXT_WINDOW",
    "CODEX_AUTO_COMPACT_TOKEN_LIMIT",
    "CODEX_AUTO_COMPACT_RATIO",
];

/// A distinctive opaque payload the mock provider "sends back"; the replay
/// assertions look for this literal on process B's outbound wire.
const OPAQUE_MARKER: &str = "XPROC-OPAQUE-7f3a9c";
const VISIBLE_REPLY: &str = "cross process visible reply";

/// Responses SSE carrying one reasoning item with opaque ciphertext plus an
/// assistant message: exactly the history shape whose replay is scoped.
const FIRST_TURN_SSE: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"rA\",\"model\":\"scope-model\"}}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"reasoning\",\"id\":\"rsn_xproc\",\"summary\":[],\"content\":[{\"type\":\"reasoning_text\",\"text\":\"visible thinking\"}],\"encrypted_content\":\"XPROC-OPAQUE-7f3a9c\"}}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"cross process visible reply\"}],\"id\":\"msg_xproc\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"rA\",\"usage\":{\"input_tokens\":9,\"output_tokens\":4,\"total_tokens\":13}}}\n\n",
);

const SECOND_TURN_SSE: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"rB\",\"model\":\"scope-model\"}}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"second turn done\"}],\"id\":\"msg_b\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"rB\",\"usage\":{\"input_tokens\":9,\"output_tokens\":3,\"total_tokens\":12}}}\n\n",
);

/// Minimal Responses gateway answering the FIRST request with the seeded
/// history SSE and every later request with the second-turn SSE, recording
/// every raw request. Producing and resuming processes share this gateway;
/// the second process still has a distinct credential-instance identity.
async fn recording_responses_gateway(
    requests: Arc<Mutex<Vec<String>>>,
) -> Result<std::net::SocketAddr> {
    async fn read_http_request(socket: &mut tokio::net::TcpStream) -> std::io::Result<String> {
        let mut data = Vec::new();
        loop {
            let mut chunk = [0u8; 4096];
            let read = socket.read(&mut chunk).await?;
            if read == 0 {
                break;
            }
            data.extend_from_slice(&chunk[..read]);
            if let Some(index) = data.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = index + 4;
                let headers = String::from_utf8_lossy(&data[..header_end]).into_owned();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then_some(value.trim())
                    })
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or_default();
                while data.len() < header_end + length {
                    let mut chunk = [0u8; 4096];
                    let read = socket.read(&mut chunk).await?;
                    if read == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..read]);
                }
                return Ok(String::from_utf8_lossy(&data).into_owned());
            }
        }
        Ok(String::from_utf8_lossy(&data).into_owned())
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let recorded = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let Ok(request) = read_http_request(&mut socket).await else {
                continue;
            };
            let index = {
                let mut guard = recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.push(request);
                guard.len()
            };
            let body = if index == 1 {
                FIRST_TURN_SSE
            } else {
                SECOND_TURN_SSE
            };
            let _ = socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await;
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.flush().await;
            let _ = socket.shutdown().await;
        }
    });
    Ok(address)
}

fn nuwax_command(
    test: &core_test_support::test_codex_exec::TestCodexExecBuilder,
    base_url: &str,
    api_key: &str,
    args: &[&str],
    prompt: &str,
) -> assert_cmd::Command {
    let repo_root = codex_utils_cargo_bin::repo_root()
        .expect("workspace root must resolve for the exec fixture");
    let mut command = test.cmd();
    for variable in CONFIG_ENV_VARIABLES {
        command.env_remove(variable);
    }
    command
        .arg("--skip-git-repo-check")
        .args(["--color", "never"])
        .arg("-C")
        .arg(&repo_root)
        .env("NUWAX_BASE_URL", base_url)
        .env("NUWAX_WIRE_API", "responses")
        .env("NUWAX_API_KEY", api_key)
        .env("NUWAX_MODEL", "scope-model")
        .env("NUWAX_REQUEST_MAX_RETRIES", "0")
        .env("NUWAX_STREAM_MAX_RETRIES", "0")
        .args(args)
        .arg(prompt)
        .timeout(Duration::from_secs(/*secs*/ 60));
    command
}

fn assert_responses_request(
    raw_request: &str,
    expected_target: &str,
    api_key: &str,
) -> Result<serde_json::Value> {
    let (headers, raw_body) = raw_request
        .split_once("\r\n\r\n")
        .context("captured request must contain HTTP headers and body")?;
    assert_eq!(
        headers.lines().next(),
        Some(format!("POST {expected_target} HTTP/1.1").as_str()),
        "the actual request must preserve the endpoint path and exact query"
    );
    let authorization = headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("authorization")
            .then_some(value.trim())
    });
    assert_eq!(authorization, Some(format!("Bearer {api_key}").as_str()));
    let body: serde_json::Value = serde_json::from_str(raw_body)?;
    assert_eq!(body["model"], "scope-model");
    Ok(body)
}

fn find_rollout(home: &Path) -> PathBuf {
    fn walk(dir: &Path) -> Option<PathBuf> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = walk(&path) {
                    return Some(found);
                }
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                return Some(path);
            }
        }
        None
    }
    walk(&home.join("sessions")).expect("at least one rollout under sessions")
}

/// Produces the seeded history in an isolated home, then runs a resume in a
/// SECOND isolated home holding a copy of that rollout, both against the
/// same gateway; the resume base may append a query suffix and rotate the
/// credential. Returns the resume request body plus the copied rollout
/// bytes before and after the resume.
async fn run_cross_process_pair(
    resume_suffix: &str,
    resume_key: &str,
) -> Result<(serde_json::Value, Vec<u8>, Vec<u8>)> {
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let address = recording_responses_gateway(requests.clone()).await?;
    let base = format!("http://{address}");

    let first = test_codex_exec();
    let first_home = first.home_path().to_path_buf();
    let first_base = base.clone();
    // Build the command here: the builder owns the temp home, so it must
    // outlive the child process (dropping it deletes the home).
    let mut first_command = nuwax_command(&first, &first_base, "scope-key-a", &[], "remember this");
    let first_output = tokio::task::spawn_blocking(move || first_command.output()).await??;
    assert!(
        first_output.status.success(),
        "producing turn must succeed: {}",
        String::from_utf8_lossy(&first_output.stderr)
    );

    // Copy the rollout into a second, isolated home.
    let source_rollout = find_rollout(&first_home);
    let source_bytes = std::fs::read(&source_rollout)?;
    assert!(
        String::from_utf8_lossy(&source_bytes).contains(OPAQUE_MARKER),
        "the producing rollout must persist the opaque ciphertext"
    );
    let second = test_codex_exec();
    let resume_key_owned = resume_key.to_string();
    let relative = source_rollout
        .strip_prefix(&first_home)
        .expect("rollout inside home");
    let target = second.home_path().join(relative);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, &source_bytes)?;

    let resume_base = format!("{base}{resume_suffix}");
    let copied_before = std::fs::read(&target)?;
    let mut second_command = nuwax_command(
        &second,
        &resume_base,
        &resume_key_owned,
        &["resume", "--last"],
        "continue in the second process",
    );
    let second_output = tokio::task::spawn_blocking(move || second_command.output()).await??;
    assert!(
        second_output.status.success(),
        "resume must succeed: {}",
        String::from_utf8_lossy(&second_output.stderr)
    );
    let captured = requests
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(captured.len(), 2, "one POST per producing/resuming process");
    assert_responses_request(&captured[0], "/responses", "scope-key-a")?;
    let resume_body = assert_responses_request(
        &captured[1],
        &format!("/responses{resume_suffix}"),
        resume_key,
    )?;
    let copied_after = std::fs::read(&target)?;
    Ok((resume_body, copied_before, copied_after))
}

fn assert_cross_process_outcome(
    label: &str,
    resume_body: &serde_json::Value,
    copied_before: &[u8],
    copied_after: &[u8],
) {
    let resume_body = serde_json::to_string(resume_body).expect("serialize request body");
    assert!(
        resume_body.contains(VISIBLE_REPLY),
        "{label}: visible history must replay on the wire"
    );
    assert!(
        resume_body.contains("visible thinking"),
        "{label}: the reasoning item's visible text must stay on the wire"
    );
    assert!(
        !resume_body.contains(OPAQUE_MARKER),
        "{label}: another process cannot prove ownership of opaque ciphertext"
    );
    assert!(
        resume_body.contains("continue in the second process"),
        "{label}: the new user turn must be on the wire"
    );
    assert!(
        copied_after.starts_with(copied_before),
        "{label}: the resume may only append; existing rollout bytes must not change"
    );
    let appended = &copied_after[copied_before.len()..];
    assert!(
        !String::from_utf8_lossy(appended).contains("scope-key-"),
        "{label}: appended rollout bytes must not embed raw credentials"
    );
}

/// Env-seeded credentials carry a
/// process-local random `credentialInstance` auth domain by design, so a
/// SECOND process can never prove credential equality: every cross-process
/// resume must degrade the opaque ciphertext while keeping the visible
/// history (including the reasoning text) replaying. The extra cells verify
/// the configured keys and query shapes reach the wire, without claiming to
/// isolate their rotation behavior from the process identity change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_process_resume_conservatively_drops_opaque_history() -> Result<()> {
    struct Cell {
        label: &'static str,
        resume_suffix: &'static str,
        resume_key: &'static str,
    }
    let cells = [
        Cell {
            label: "same key (process-local identity still degrades)",
            resume_suffix: "",
            resume_key: "scope-key-a",
        },
        Cell {
            label: "rotated credential",
            resume_suffix: "",
            resume_key: "scope-key-b",
        },
        Cell {
            label: "unknown query name",
            resume_suffix: "?sessionkey=xyz",
            resume_key: "scope-key-a",
        },
        Cell {
            label: "duplicate query",
            resume_suffix: "?a=1&a=2",
            resume_key: "scope-key-a",
        },
        Cell {
            label: "empty query value",
            resume_suffix: "?q=",
            resume_key: "scope-key-a",
        },
    ];

    for cell in cells {
        let (resume_body, copied_before, copied_after) =
            run_cross_process_pair(cell.resume_suffix, cell.resume_key).await?;
        assert_cross_process_outcome(cell.label, &resume_body, &copied_before, &copied_after);
    }
    Ok(())
}
