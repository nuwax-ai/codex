//! Scene assertions share one failure-retention boundary after execution.

use super::*;
use anyhow::Context;
use runner::ExecRunner;
use runner::TurnOutcomeExpectation;
use runner::execute;
use runner::execute_expecting;

pub(super) struct Scene<'a> {
    pub(super) prepared: &'a PreparedExec,
    pub(super) home: &'a Path,
    pub(super) cwd: &'a Path,
    pub(super) artifacts: &'a Path,
    pub(super) protocol: &'a str,
    pub(super) marker: &'a str,
    /// The wire facts every captured attempt must carry: the model and the
    /// final URL prefix of the provider this scene was configured against.
    pub(super) expected_model: &'a str,
    pub(super) expected_url_prefix: &'a str,
    pub(super) wire: codex_rust_rig_bridge::RigProtocol,
    pub(super) capture_requirement: super::capture_validation::CaptureRequirement,
    pub(super) expected_cap: super::capture_validation::CapExpectation,
}

pub(super) enum BinaryScenario<'a> {
    Marker { expect_bridge_log: Option<&'a str> },
    Compact,
    WebSearch,
    CapExhausted,
}

pub(super) async fn run(
    runner: &impl ExecRunner,
    scene: &Scene<'_>,
    scenario: BinaryScenario<'_>,
) -> Result<()> {
    let cap_exhaustion = matches!(scenario, BinaryScenario::CapExhausted);
    let outcome = match scenario {
        BinaryScenario::Marker { expect_bridge_log } => {
            marker(runner, scene, expect_bridge_log).await
        }
        BinaryScenario::Compact => compact(runner, scene).await,
        BinaryScenario::WebSearch => websearch(runner, scene).await,
        BinaryScenario::CapExhausted => cap_exhausted(runner, scene).await,
    };
    // Capture presence is recorded separately from wire assertions. Native
    // transports and injected runners can legitimately produce no Rig trace.
    let capture_file = scene.artifacts.join("requests.jsonl");
    let mut capture_error = None;
    let (status, attempts, asserted_fields_raw) = match std::fs::read_to_string(&capture_file) {
        Ok(contents) => {
            let lines: Vec<_> = contents
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            let parsed = lines
                .iter()
                .map(|line| serde_json::from_str::<Value>(line))
                .collect::<std::result::Result<Vec<_>, _>>();
            if lines.is_empty() {
                capture_error = Some(anyhow!("request capture has no HTTP attempts"));
                ("empty", 0, Vec::new())
            } else if let Ok(attempts) = parsed {
                // Wire-level evidence: each recorded final attempt must hit
                // the configured provider with the configured model. This is
                // the pre-send capture, not proof of server receipt.
                let mut asserted_fields: Vec<&'static str> = Vec::new();
                let mut wire: anyhow::Result<()> = Ok(());
                for attempt in &attempts {
                    let attempt_wire = super::capture_validation::validate_attempt(
                        attempt,
                        scene.expected_url_prefix,
                        scene.wire,
                        scene.expected_model,
                    )
                    .and(
                        super::capture_validation::validate_cap(
                            attempt,
                            scene.wire,
                            scene.expected_cap,
                        )
                        .map_err(|error| anyhow::anyhow!("output cap: {error:#}")),
                    );
                    match attempt_wire {
                        Ok(cap_field) => asserted_fields.push(cap_field),
                        Err(error) => {
                            wire = Err(error);
                            break;
                        }
                    }
                }
                match wire {
                    Ok(()) => {
                        asserted_fields.dedup();
                        ("available", attempts.len(), asserted_fields)
                    }
                    Err(error) => {
                        capture_error = Some(error.context("wire field assertion failed"));
                        ("field_mismatch", attempts.len(), Vec::new())
                    }
                }
            } else {
                capture_error = Some(anyhow!("request capture contains malformed JSON"));
                ("malformed", lines.len(), Vec::new())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if matches!(
                scene.capture_requirement,
                super::capture_validation::CaptureRequirement::Required
            ) {
                capture_error = Some(anyhow!("required Rig request capture is absent"));
            }
            ("absent", 0, Vec::new())
        }
        Err(error) => {
            capture_error = Some(anyhow!(error).context("read executable request capture"));
            ("read_error", 0, Vec::new())
        }
    };
    let mut asserted_fields: Vec<String> = asserted_fields_raw
        .into_iter()
        .map(str::to_string)
        .collect();
    if status != "available" {
        asserted_fields.clear();
    } else {
        let mut base = ["url.scheme", "url.authority", "url.path", "body.model"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        base.append(&mut asserted_fields);
        asserted_fields = base;
    }
    let evidence = serde_json::json!({ "read_status": status, "http_attempts": attempts,
        "wire_asserted": status == "available",
        "asserted_fields": asserted_fields,
        "path": "requests.jsonl" });
    let persist = (|| -> Result<()> {
        std::fs::write(
            scene.artifacts.join("request-capture-evidence.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
        Ok(())
    })();
    if cap_exhaustion && capture_error.is_none() && attempts != 1 {
        capture_error = Some(anyhow!(
            "an exhausted cap must issue exactly one HTTP attempt without resampling; captured {attempts}"
        ));
    }
    let capture_outcome = match capture_error {
        Some(error) => Err(error),
        None => persist,
    };
    let outcome = match (outcome, capture_outcome) {
        (Err(error), Err(capture_error)) => {
            eprintln!("warn: request capture evidence: {capture_error:#}");
            Err(error)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), result) => result,
    };
    if cap_exhaustion && outcome.is_ok() {
        rollouts::retain_best_effort(scene.home, &scene.artifacts.join("rollout"))
            .context("retain capped-out turn rollout")?;
    }
    retain_rollouts_on_failure(scene.home, &scene.artifacts.join("rollout"), outcome)
}

async fn marker(
    runner: &impl ExecRunner,
    scene: &Scene<'_>,
    expect_bridge_log: Option<&str>,
) -> Result<()> {
    let prompt = format!(
        "请务必调用 shell 工具真实执行命令 `echo {}`（不要只把命令当文本输出），然后把命令的原始输出逐字告诉我，不要添加任何解释。",
        scene.marker
    );
    let last = scene.home.join("last_message.txt");
    let (stdout, stderr) = execute(runner, scene, &[], &prompt, &last, "").await?;
    let final_message =
        std::fs::read_to_string(&last).context("marker turn produced no final answer file")?;
    std::fs::write(scene.artifacts.join("final_message.txt"), &final_message)?;
    anyhow::ensure!(
        !final_message.trim().is_empty(),
        "marker turn produced no final answer"
    );
    if let Some(expected) = expect_bridge_log {
        anyhow::ensure!(
            stderr.contains(expected),
            "expected dispatch log `{expected}` on stderr: {stderr}"
        );
    }
    let executed = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|event| {
            let item = event.get("item").unwrap_or(&event);
            event["type"] == "item.completed"
                && item["type"] == "command_execution"
                && item["exit_code"].as_i64() == Some(0)
                && item["aggregated_output"]
                    .as_str()
                    .is_some_and(|output| output.contains(scene.marker))
        });
    anyhow::ensure!(
        executed,
        "executed command must complete with exit 0 and the marker in its output"
    );
    println!(
        "[{}] OK marker={} final_message_chars={}",
        scene.protocol,
        scene.marker,
        final_message.chars().count()
    );
    Ok(())
}

/// Live cap EXHAUSTION (D5): an output budget small enough that the REAL
/// vendor truncates the turn (Chat `finish_reason=length`). The shared
/// capture validation asserts the tiny cap on the wire per attempt; here the
/// process must fail through the product's cap terminal — partial streaming
/// may exist, but no successful turn completes.
async fn cap_exhausted(runner: &impl ExecRunner, scene: &Scene<'_>) -> Result<()> {
    let prompt = "请直接输出一段至少两百字的故事，主题是远航的灯塔，不要任何解释。";
    let last = scene.home.join("last_message.txt");
    let (_, stderr) = execute_expecting(
        runner,
        scene,
        &[],
        prompt,
        &last,
        "",
        TurnOutcomeExpectation::Failure,
    )
    .await?;
    let needle = "Output token limit reached";
    anyhow::ensure!(
        stderr.contains(needle),
        "an exhausted cap must surface the product's cap terminal on stderr: {stderr}"
    );
    let stdout = std::fs::read_to_string(scene.artifacts.join("events.jsonl"))
        .context("read capped-out turn events as UTF-8")?;
    let events: Vec<Value> = stdout
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str::<Value>(line)
                .with_context(|| format!("parse capped-out turn event on line {}", index + 1))
        })
        .collect::<Result<_>>()?;
    anyhow::ensure!(
        events.iter().any(|event| event["type"] == "turn.failed"
            && event["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(needle))),
        "a capped-out turn must fail with the cap terminal event: {stdout}"
    );
    anyhow::ensure!(
        !events.iter().any(|event| event["type"] == "turn.completed"),
        "a capped-out turn must not report a completed turn: {stdout}"
    );
    println!(
        "[{}] OK cap-exhausted stderr_names_terminal=true final_message_absent={}",
        scene.protocol,
        !last.exists()
    );
    Ok(())
}

async fn compact(runner: &impl ExecRunner, scene: &Scene<'_>) -> Result<()> {
    let last1 = scene.home.join("last_message_1.txt");
    execute(
        runner,
        scene,
        &[],
        &format!("请记住暗号：{}。只回复OK。", scene.marker),
        &last1,
        "turn1",
    )
    .await?;
    let first =
        std::fs::read_to_string(&last1).context("compact turn 1 produced no final answer file")?;
    anyhow::ensure!(
        !first.trim().is_empty(),
        "compact turn 1 produced no final answer"
    );
    let last2 = scene.home.join("last_message_2.txt");
    execute(
        runner,
        scene,
        &["resume", "--last"],
        "暗号是什么？直接回答暗号本身，不要任何其他内容。",
        &last2,
        "turn2",
    )
    .await?;
    let answer =
        std::fs::read_to_string(&last2).context("compact turn 2 produced no final answer file")?;
    anyhow::ensure!(
        answer.contains(scene.marker),
        "passphrase lost after compaction; answer: {answer}"
    );
    let paths = rollouts::session_rollouts(scene.home);
    anyhow::ensure!(!paths.is_empty(), "no session rollout for compaction");
    let mut found = false;
    for path in paths {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("read rollout {}", path.display()))?;
        let mut compacted = false;
        for line in contents.lines().filter(|line| !line.trim().is_empty()) {
            let record: Value =
                serde_json::from_str(line).context("parse compaction rollout record")?;
            if record["type"] == "compacted" && line.contains(scene.marker) {
                compacted = true;
            } else if compacted
                && record["type"] == "response_item"
                && record["payload"]["type"] == "message"
                && record["payload"]["role"] == "assistant"
                && line.contains(scene.marker)
            {
                found = true;
            }
        }
    }
    anyhow::ensure!(
        found,
        "recall answer must follow a compacted record carrying the passphrase in the same rollout"
    );
    rollouts::retain_best_effort(scene.home, &scene.artifacts.join("rollout"))?;
    println!(
        "[{}] OK compact marker={} answer_chars={}",
        scene.protocol,
        scene.marker,
        answer.chars().count()
    );
    Ok(())
}

async fn websearch(runner: &impl ExecRunner, scene: &Scene<'_>) -> Result<()> {
    let last1 = scene.home.join("last_message_1.txt");
    execute(
        runner,
        scene,
        &[],
        "用 web 搜索今天北京的天气，然后用一句话总结。",
        &last1,
        "turn1",
    )
    .await?;
    let answer1 = std::fs::read_to_string(&last1)
        .context("web-search turn 1 produced no final answer file")?;
    anyhow::ensure!(
        !answer1.trim().is_empty(),
        "web-search turn 1 produced no answer"
    );
    let first = rollouts::search_evidence(scene.home)?;
    anyhow::ensure!(
        first.completed_pairs >= 1,
        "turn 1 answered without a completed matched web_search_call/result"
    );
    let last2 = scene.home.join("last_message_2.txt");
    execute(runner, scene, &["resume", "--last"],
        "本轮请必须先调用已提供的服务端 web_search 工具，重新搜索今天上海的天气，再用一句话总结。不能只根据上轮历史或已有知识回答，不能用 shell/curl 代替服务端搜索；搜索失败时请如实说明，不能编造结果。",
        &last2, "turn2").await?;
    let answer2 = std::fs::read_to_string(&last2)
        .context("web-search turn 2 produced no final answer file")?;
    anyhow::ensure!(
        !answer2.trim().is_empty(),
        "web-search turn 2 produced no answer"
    );
    let second = rollouts::retain_search_rollouts(scene.home, &scene.artifacts.join("rollout"))?;
    anyhow::ensure!(
        second.completed_pairs > first.completed_pairs,
        "turn 2 added no completed matched web_search_call/result"
    );
    let evidence = serde_json::json!({
        "turn1": { "answer_chars": answer1.chars().count(),
            "completed_matched_pairs": first.completed_pairs, "citations": first.citations },
        "turn2": { "answer_chars": answer2.chars().count(),
            "completed_matched_pairs": second.completed_pairs - first.completed_pairs,
            "citations": second.citations.saturating_sub(first.citations) },
        "citation_capability_asserted": false,
    });
    std::fs::write(
        scene.artifacts.join("search-evidence.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    println!("[{}] OK websearch {evidence}", scene.protocol);
    Ok(())
}
