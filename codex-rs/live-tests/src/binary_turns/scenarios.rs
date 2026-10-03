//! Scene assertions share one failure-retention boundary after execution.

use super::*;
use anyhow::Context;
use runner::ExecRunner;
use runner::execute;

pub(super) struct Scene<'a> {
    pub(super) prepared: &'a PreparedExec,
    pub(super) home: &'a Path,
    pub(super) cwd: &'a Path,
    pub(super) artifacts: &'a Path,
    pub(super) protocol: &'a str,
    pub(super) marker: &'a str,
}

pub(super) enum BinaryScenario<'a> {
    Marker { expect_bridge_log: Option<&'a str> },
    Compact,
    WebSearch,
}

pub(super) async fn run(
    runner: &impl ExecRunner,
    scene: &Scene<'_>,
    scenario: BinaryScenario<'_>,
) -> Result<()> {
    let outcome = match scenario {
        BinaryScenario::Marker { expect_bridge_log } => {
            marker(runner, scene, expect_bridge_log).await
        }
        BinaryScenario::Compact => compact(runner, scene).await,
        BinaryScenario::WebSearch => websearch(runner, scene).await,
    };
    // Capture presence is recorded separately from wire assertions. Native
    // transports and injected runners can legitimately produce no Rig trace.
    let capture_file = scene.artifacts.join("requests.jsonl");
    let mut capture_error = None;
    let (status, attempts) = match std::fs::read_to_string(&capture_file) {
        Ok(contents) => {
            let lines: Vec<_> = contents
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            if lines
                .iter()
                .all(|line| serde_json::from_str::<Value>(line).is_ok())
            {
                ("available", lines.len())
            } else {
                capture_error = Some(anyhow!("request capture contains malformed JSON"));
                ("malformed", lines.len())
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ("absent", 0),
        Err(error) => {
            capture_error = Some(anyhow!(error).context("read executable request capture"));
            ("read_error", 0)
        }
    };
    let evidence = serde_json::json!({ "read_status": status, "http_attempts": attempts,
        "wire_asserted": false, "path": "requests.jsonl" });
    let persist = (|| -> Result<()> {
        std::fs::write(
            scene.artifacts.join("request-capture-evidence.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
        Ok(())
    })();
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
