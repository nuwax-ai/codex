//! Durable completed search evidence and verbatim rollout retention.

use super::*;
use anyhow::Context;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SearchEvidence {
    pub(super) completed_pairs: usize,
    pub(super) citations: usize,
}

/// Attempts every copy. Callers on a failure path report this error separately
/// so neither malformed JSON nor retention I/O replaces the original failure.
pub(super) fn retain_best_effort(home: &Path, rollout_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(rollout_dir)?;
    let mut first_error = None;
    for path in session_rollouts(home) {
        let Some(name) = path.file_name() else {
            continue;
        };
        if let Err(error) = std::fs::copy(&path, rollout_dir.join(name)) {
            eprintln!("warn: retain rollout {}: {error}", path.display());
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn retain_search_rollouts(home: &Path, rollout_dir: &Path) -> Result<SearchEvidence> {
    // Copy all bytes before parsing, including later files after a corrupt line.
    retain_best_effort(home, rollout_dir)?;
    let retained: Vec<_> = session_rollouts(home)
        .iter()
        .filter_map(|path| path.file_name().map(|name| rollout_dir.join(name)))
        .collect();
    let evidence = inspect_paths(&retained)?;
    anyhow::ensure!(
        evidence.completed_pairs > 0,
        "no persisted completed matched search envelope"
    );
    Ok(evidence)
}

pub(super) fn search_evidence(home: &Path) -> Result<SearchEvidence> {
    inspect_paths(&session_rollouts(home))
}

#[derive(Default)]
struct ResponseIdentity {
    indices: BTreeMap<u64, Value>,
    layout: Option<Vec<Value>>,
}

fn inspect_paths(paths: &[PathBuf]) -> Result<SearchEvidence> {
    let mut calls = BTreeSet::new();
    let mut citations = BTreeSet::new();
    let mut responses = BTreeMap::<(String, String), ResponseIdentity>::new();
    for path in paths {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("read rollout {}", path.display()))?;
        for (index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: Value = serde_json::from_str(line)
                .with_context(|| format!("{} line {}", path.display(), index + 1))?;
            let payload = &record["payload"];
            if record["type"] != "response_item"
                || payload["type"] != "web_search_call"
                || payload["status"] != "completed"
            {
                continue;
            }
            inspect_envelope(
                &payload["wire_blocks"],
                &mut calls,
                &mut citations,
                &mut responses,
            )
            .with_context(|| {
                format!(
                    "{} line {}: invalid search replay envelope",
                    path.display(),
                    index + 1
                )
            })?;
        }
    }
    for identity in responses.values() {
        if let Some(layout) = &identity.layout {
            for entry in layout {
                if entry["kind"] == "pair" {
                    let index = entry["index"]
                        .as_u64()
                        .ok_or_else(|| anyhow!("pair layout index missing"))?;
                    anyhow::ensure!(
                        identity.indices.contains_key(&index),
                        "layout references missing response block identity {index}"
                    );
                }
            }
        }
    }
    Ok(SearchEvidence {
        completed_pairs: calls.len(),
        citations: citations.len(),
    })
}

fn inspect_envelope(
    envelope: &Value,
    calls: &mut BTreeSet<(String, String)>,
    citations: &mut BTreeSet<String>,
    responses: &mut BTreeMap<(String, String), ResponseIdentity>,
) -> Result<()> {
    let version = envelope["version"]
        .as_u64()
        .ok_or_else(|| anyhow!("missing envelope version"))?;
    anyhow::ensure!(
        matches!(version, 1..=3),
        "unsupported envelope version {version}"
    );
    let source = envelope["source"]
        .as_str()
        .filter(|source| !source.trim().is_empty())
        .ok_or_else(|| anyhow!("missing envelope source"))?;
    let blocks = envelope["blocks"]
        .as_array()
        .ok_or_else(|| anyhow!("search blocks must be an array"))?;
    anyhow::ensure!(!blocks.is_empty(), "search blocks are empty");
    anyhow::ensure!(
        serde_json::to_vec(envelope)?.len() <= 9_800,
        "search envelope exceeds byte cap"
    );
    let mut local_calls = BTreeMap::new();
    let mut results = BTreeSet::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("server_tool_use") => {
                let id = block["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow!("search call id missing"))?;
                // Match the production capture's hosted web-search family;
                // GLM preserves its gateway name `web_search_prime`.
                anyhow::ensure!(
                    block["name"]
                        .as_str()
                        .is_some_and(|name| name.contains("web_search"))
                        && block["input"].is_object(),
                    "search call name/input invalid"
                );
                anyhow::ensure!(
                    local_calls.insert(id, block).is_none(),
                    "duplicate search call id {id}"
                );
            }
            Some("web_search_tool_result" | "tool_result") => {
                let id = block["tool_use_id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow!("search result id missing"))?;
                let content = &block["content"];
                let valid_content = if block["type"] == "web_search_tool_result" {
                    content.is_array() || content.is_object()
                } else {
                    content.is_array() || content.is_string()
                };
                anyhow::ensure!(
                    valid_content && results.insert(id),
                    "search result content/identity invalid"
                );
            }
            _ => anyhow::bail!("unexpected search block type"),
        }
    }
    anyhow::ensure!(
        !local_calls.is_empty() && local_calls.keys().copied().collect::<BTreeSet<_>>() == results,
        "completed search envelope must contain matching call/result pairs"
    );
    for id in local_calls.keys() {
        calls.insert((source.to_string(), (*id).to_string()));
    }
    let mut response_id = None;
    if version >= 2 {
        let indices = envelope["block_indices"]
            .as_array()
            .ok_or_else(|| anyhow!("block_indices missing"))?;
        anyhow::ensure!(
            indices.len() == blocks.len(),
            "block_indices length mismatch"
        );
        let mut unique = BTreeSet::new();
        let mut previous = None;
        for (value, block) in indices.iter().zip(blocks) {
            let index = value
                .as_u64()
                .ok_or_else(|| anyhow!("block index must be unsigned"))?;
            if index == u64::MAX {
                anyhow::ensure!(
                    block["type"] == "server_tool_use",
                    "unknown result block identity"
                );
            } else {
                anyhow::ensure!(
                    unique.insert(index) && previous.is_none_or(|prior| prior < index),
                    "duplicate/unordered block identity"
                );
                previous = Some(index);
            }
        }
        if version == 3 {
            let id = envelope["response_id"]
                .as_str()
                .filter(|id| {
                    !id.is_empty()
                        && id.len() <= 128
                        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
                })
                .ok_or_else(|| anyhow!("response_id missing"))?;
            response_id = Some(id);
            let identity = responses
                .entry((source.to_string(), id.to_string()))
                .or_default();
            for (index, block) in indices.iter().zip(blocks) {
                let index = index
                    .as_u64()
                    .ok_or_else(|| anyhow!("invalid block index"))?;
                if index != u64::MAX
                    && let Some(previous) = identity.indices.insert(index, block.clone())
                {
                    anyhow::ensure!(previous == *block, "conflicting response block identity");
                }
            }
        }
    }
    let cited = if let Some(layout) = envelope.get("layout") {
        anyhow::ensure!(version >= 2, "v1 must not claim layout identity");
        let layout = layout
            .as_array()
            .ok_or_else(|| anyhow!("layout must be an array"))?;
        let citation_identity = response_id
            .map(str::to_string)
            .unwrap_or_else(|| local_calls.keys().copied().collect::<Vec<_>>().join(","));
        let mut indices = BTreeSet::new();
        let mut previous = None;
        let mut segment_parts = BTreeMap::<&str, u64>::new();
        for entry in layout {
            let index = entry["index"]
                .as_u64()
                .ok_or_else(|| anyhow!("layout index missing"))?;
            anyhow::ensure!(
                index != u64::MAX
                    && indices.insert(index)
                    && previous.is_none_or(|prior| prior < index),
                "duplicate/unordered layout identity"
            );
            previous = Some(index);
            if version == 3 && entry["kind"] != "pair" {
                let owner = entry["owner"]
                    .as_str()
                    .ok_or_else(|| anyhow!("layout owner missing"))?;
                let rest = owner
                    .strip_prefix("rigseg_")
                    .ok_or_else(|| anyhow!("layout owner malformed"))?;
                let (response, ordinal) = rest
                    .rsplit_once('_')
                    .ok_or_else(|| anyhow!("layout owner malformed"))?;
                anyhow::ensure!(
                    Some(response) == response_id && ordinal.parse::<usize>().is_ok(),
                    "layout owner belongs to another response"
                );
            }
            match entry["kind"].as_str() {
                Some("text" | "cited") => {
                    anyhow::ensure!(
                        entry["block"]["type"] == "text" && entry["block"]["text"].is_string(),
                        "layout text block invalid"
                    );
                    if entry["kind"] == "cited" {
                        validate_citations(
                            &entry["block"],
                            &format!("{source}:{citation_identity}:{index}"),
                            citations,
                        )?;
                    }
                }
                Some("pair") => {}
                Some("segment") if version == 3 => {
                    let owner = entry["owner"]
                        .as_str()
                        .ok_or_else(|| anyhow!("segment owner missing"))?;
                    let part = entry["part"]
                        .as_u64()
                        .ok_or_else(|| anyhow!("segment part missing"))?;
                    let expected = segment_parts.entry(owner).or_default();
                    anyhow::ensure!(part == *expected, "segment part order invalid");
                    *expected += 1;
                }
                _ => anyhow::bail!("unknown layout kind"),
            }
        }
        if let Some(id) = response_id {
            let identity = responses
                .entry((source.to_string(), id.to_string()))
                .or_default();
            if let Some(previous) = identity.layout.replace(layout.to_vec()) {
                anyhow::ensure!(previous == *layout, "conflicting response layout identity");
            }
        }
        None
    } else {
        envelope.get("cited_text")
    };
    if let Some(cited) = cited {
        let cited = cited
            .as_array()
            .ok_or_else(|| anyhow!("cited_text must be an array"))?;
        for (index, block) in cited.iter().enumerate() {
            validate_citations(
                block,
                &format!(
                    "{source}:{}:{index}",
                    local_calls.keys().copied().collect::<Vec<_>>().join(",")
                ),
                citations,
            )?;
        }
    }
    Ok(())
}

fn validate_citations(block: &Value, owner: &str, citations: &mut BTreeSet<String>) -> Result<()> {
    anyhow::ensure!(
        block["type"] == "text" && block["text"].is_string(),
        "cited text block malformed"
    );
    let entries = block["citations"]
        .as_array()
        .ok_or_else(|| anyhow!("citations must be an array"))?;
    for (index, entry) in entries.iter().enumerate() {
        anyhow::ensure!(
            entry.is_object() && entry["type"].as_str().is_some_and(|kind| !kind.is_empty()),
            "citation malformed"
        );
        citations.insert(format!("{owner}:{index}:{}", serde_json::to_string(entry)?));
    }
    Ok(())
}

pub(super) fn session_rollouts(home: &Path) -> Vec<PathBuf> {
    fn visit(dir: &Path, paths: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, paths);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
            {
                paths.push(path);
            }
        }
    }
    let mut paths = Vec::new();
    visit(&home.join("sessions"), &mut paths);
    paths.sort();
    paths
}
