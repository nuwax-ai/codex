//! Persist complete provider reasoning blocks, including signatures and redactions.
//! The versioned envelope is opaque to Codex and is only replayed to its original
//! endpoint, protocol and model. Native encrypted Responses payloads are never text.

use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use rig_core::completion::message::AssistantContent;
use rig_core::completion::message::Native;
use rig_core::completion::message::Reasoning;

/// Marker prefix of the original (rig 0.42) reasoning envelope. Kept for
/// decoding old rollouts; new envelopes use [`REPLAY_PREFIX_V2`]. Public
/// because core's request-copy filter strips both envelopes before
/// dispatching to the native or genai transports.
pub const REPLAY_PREFIX: &str = "codex-rig-reasoning-v1:";

/// Marker prefix of the rig 0.44 reasoning envelope: `Reasoning` gained a
/// new shape (`text`/`redacted`/`native`), so new recordings cannot be
/// written into v1 payloads without lying about their schema.
pub const REPLAY_PREFIX_V2: &str = "codex-rig-reasoning-v2:";

/// Whether an `encrypted_content` value holds this bridge's replay envelope
/// (as opposed to native Responses ciphertext or legacy duplicated text).
pub fn is_replay_envelope(value: &str) -> bool {
    value.starts_with(REPLAY_PREFIX) || value.starts_with(REPLAY_PREFIX_V2)
}

/// The rig 0.42 reasoning block shapes recorded inside v1 envelopes. The
/// legacy DTO is decode-only: old rollouts are never rewritten in place,
/// and their blocks convert to the 0.44 `Reasoning` on read.
/// Pinned rig 0.42 serde shape: adjacently tagged with the payload nested
/// under `content` (`{"type":"text","content":{"text":..,"signature":..}}`).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
enum LegacyReasoningContent {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    Encrypted(String),
    Redacted {
        data: String,
    },
    Summary(String),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct LegacyReasoning {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    content: Vec<LegacyReasoningContent>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Replay {
    source: String,
    #[serde(default)]
    blocks: Vec<Reasoning>,
    /// v1 payloads carried `Vec<LegacyReasoning>` in `blocks`; the field is
    /// kept untagged-compatible by decoding legacy blocks separately below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_blocks: Option<Vec<LegacyReasoning>>,
}

impl Replay {
    /// The blocks in rig 0.44 form, converting legacy v1 blocks on read.
    fn into_blocks(self, protocol: crate::RigProtocol) -> Vec<Reasoning> {
        if let Some(legacy) = self.legacy_blocks {
            return legacy_replay_blocks(legacy, protocol);
        }
        self.blocks
    }
}

/// One 0.44 block for each 0.42 content part: a signature applies only to
/// its own text, and redacted or encrypted parts retain their own payload.
fn convert_legacy_reasoning(block: LegacyReasoning) -> Vec<Reasoning> {
    block
        .content
        .into_iter()
        .map(|part| {
            let (mut reasoning, item) = match part {
                LegacyReasoningContent::Text { text, signature } => {
                    let item = signature.map(|signature| {
                        serde_json::json!({"type":"thinking","thinking":text,"signature":signature})
                    });
                    (Reasoning::new(text), item)
                }
                LegacyReasoningContent::Encrypted(data)
                | LegacyReasoningContent::Redacted { data } => (
                    Reasoning {
                        redacted: true,
                        ..Reasoning::default()
                    },
                    Some(serde_json::json!({"type":"redacted_thinking","data":data})),
                ),
                // 0.42 replayed summaries as unsigned thinking, but did not
                // project them into Codex's visible reasoning content.
                LegacyReasoningContent::Summary(text) => (
                    Reasoning {
                        redacted: true,
                        ..Reasoning::default()
                    },
                    Some(serde_json::json!({"type":"thinking","thinking":text})),
                ),
            };
            if let Some(item) = item {
                reasoning.native = Some(Native {
                    item,
                    fingerprint: AssistantContent::Reasoning(reasoning.clone()).fingerprint(),
                });
            }
            reasoning
        })
        .collect()
}

fn legacy_replay_blocks(
    blocks: Vec<LegacyReasoning>,
    protocol: crate::RigProtocol,
) -> Vec<Reasoning> {
    blocks
        .into_iter()
        .flat_map(convert_legacy_reasoning)
        .map(|mut reasoning| {
            // The old Messages encoder sent unsigned text as thinking too.
            // Chat gives that text its own reasoning_content field instead.
            if protocol == crate::RigProtocol::Anthropic && reasoning.native.is_none() {
                reasoning.native = Some(Native {
                    item: serde_json::json!({"type":"thinking","thinking":reasoning.text}),
                    fingerprint: AssistantContent::Reasoning(reasoning.clone()).fingerprint(),
                });
            }
            reasoning
        })
        .collect()
}

/// Converts one legacy (0.42) `Reasoning` JSON object — as recorded inside
/// v1 envelopes or legacy cassettes — into the 0.44 shape. Shared by the
/// envelope decode and the cassette translation.
pub(crate) fn convert_legacy_reasoning_value(
    value: &serde_json::Value,
) -> Result<Vec<Reasoning>, codex_api::ApiError> {
    let legacy: LegacyReasoning = serde_json::from_value(value.clone()).map_err(|error| {
        codex_api::ApiError::Stream(format!("invalid legacy reasoning block: {error}"))
    })?;
    Ok(convert_legacy_reasoning(legacy))
}

#[derive(Default)]
pub(crate) struct ReasoningState {
    ids: Vec<String>,
    blocks: Vec<Reasoning>,
}

impl ReasoningState {
    pub(crate) fn delta(&mut self, id: String, text: &str) -> usize {
        let index = match self.ids.iter().position(|existing| existing == &id) {
            Some(index) => index,
            None => {
                self.ids.push(id);
                self.blocks.push(Reasoning::new(""));
                self.blocks.len() - 1
            }
        };
        let block = &mut self.blocks[index];
        if block.native.is_none() && !block.redacted {
            block.text.push_str(text);
        } else {
            // Out of rig's contract: a delta after the complete Reasoning
            // for this id replaced the block (completion supersedes deltas).
            tracing::debug!(id = %self.ids[index], "dropping reasoning delta after its completed block");
        }
        // The visible content index counts only blocks that hold visible
        // text: redacted blocks stay in the envelope for replay but occupy
        // no visible slot, so they must not shift the indices.
        self.blocks
            .iter()
            .take(index)
            .filter(|b| !b.redacted)
            .count()
    }

    pub(crate) fn complete(&mut self, id: String, reasoning: Reasoning) {
        if let Some(index) = self.ids.iter().position(|existing| existing == &id) {
            self.blocks[index] = reasoning;
        } else {
            self.ids.push(id);
            self.blocks.push(reasoning);
        }
    }

    pub(crate) fn finish(&mut self, source: &str) -> Option<(Vec<ReasoningItemContent>, String)> {
        if self.blocks.is_empty() {
            return None;
        }
        let blocks = std::mem::take(&mut self.blocks);
        self.ids.clear();
        let content = blocks
            .iter()
            .filter(|reasoning| !reasoning.redacted)
            .map(|reasoning| ReasoningItemContent::ReasoningText {
                text: reasoning.text.clone(),
            })
            .collect();
        // Only strings and Vecs are serialized here; no non-string map keys or floats.
        match serde_json::to_string(&Replay {
            source: source.to_string(),
            blocks,
            legacy_blocks: None,
        }) {
            Ok(encoded) => Some((content, format!("{REPLAY_PREFIX_V2}{encoded}"))),
            Err(error) => {
                tracing::error!(%error, "Failed to preserve Rig reasoning");
                None
            }
        }
    }
}

pub(crate) fn replay_reasoning(
    item: &ResponseItem,
    source: &str,
    protocol: crate::RigProtocol,
) -> Vec<Reasoning> {
    let ResponseItem::Reasoning {
        encrypted_content,
        content,
        ..
    } = item
    else {
        return Vec::new();
    };
    if let Some((encoded, legacy)) = encrypted_content.as_deref().and_then(|value| {
        value
            .strip_prefix(REPLAY_PREFIX)
            .map(|encoded| (encoded, true))
            .or_else(|| {
                value
                    .strip_prefix(REPLAY_PREFIX_V2)
                    .map(|encoded| (encoded, false))
            })
    }) {
        if legacy {
            // v1 envelopes recorded rig 0.42 blocks; decode the legacy DTO
            // rather than pretending the 0.44 shape was always there.
            #[derive(serde::Deserialize)]
            struct LegacyReplay {
                source: String,
                blocks: Vec<LegacyReasoning>,
            }
            return match serde_json::from_str::<LegacyReplay>(encoded) {
                Ok(replay) if replay.source == source => {
                    legacy_replay_blocks(replay.blocks, protocol)
                }
                Ok(_) => {
                    tracing::warn!(
                        "Dropping reasoning envelope recorded for a different endpoint/model"
                    );
                    Vec::new()
                }
                Err(error) => {
                    tracing::warn!(%error, "Ignoring invalid Rig reasoning replay envelope");
                    Vec::new()
                }
            };
        }
        return match serde_json::from_str::<Replay>(encoded) {
            Ok(replay) if replay.source == source => replay.into_blocks(protocol),
            Ok(replay) => {
                // Recorded against a different endpoint/protocol/model —
                // e.g. the user switched provider mid-session. Reasoning
                // replay is silently invalid cross-source; make the loss
                // visible for troubleshooting.
                tracing::warn!(
                    envelope_source = %replay.source,
                    "Dropping reasoning envelope recorded for a different endpoint/model"
                );
                Vec::new()
            }
            Err(error) => {
                tracing::warn!(%error, "Ignoring invalid Rig reasoning replay envelope");
                Vec::new()
            }
        };
    }
    // Chat accepts visible reasoning_content without a signature. The Core
    // source projection may deliberately remove opaque bytes while retaining
    // that text; losing it breaks providers that require reasoning on tool
    // continuations. Anthropic still requires its original signed envelope.
    let text: String = content
        .iter()
        .flatten()
        .map(|part| match part {
            ReasoningItemContent::ReasoningText { text } | ReasoningItemContent::Text { text } => {
                text.as_str()
            }
        })
        .collect();
    let projected_plaintext = encrypted_content.is_none();
    if projected_plaintext && text.len() > crate::hosted_replay::MAX_PAIR_BYTES {
        tracing::warn!("Dropping projected plaintext reasoning above the replay byte budget");
        return Vec::new();
    }
    if protocol == crate::RigProtocol::Chat
        && !text.is_empty()
        && (projected_plaintext || encrypted_content.as_deref() == Some(text.as_str()))
    {
        if projected_plaintext && text.len() >= 1_000 {
            tracing::warn!(
                bytes = text.len(),
                "P0 manual review: projected reasoning can exceed 1k tokens"
            );
        }
        vec![Reasoning::new(&text)]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
#[path = "reasoning_projection_tests.rs"]
mod projection_tests;
