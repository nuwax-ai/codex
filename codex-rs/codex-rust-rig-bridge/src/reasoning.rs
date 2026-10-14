//! Persist complete provider reasoning blocks, including signatures and redactions.
//! The versioned envelope is opaque to Codex and is only replayed to its original
//! endpoint, protocol and model. Native encrypted Responses payloads are never text.

use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
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
#[derive(serde::Serialize, serde::Deserialize)]
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
    fn into_blocks(self) -> Vec<Reasoning> {
        if let Some(legacy) = self.legacy_blocks {
            return legacy.into_iter().map(convert_legacy_reasoning).collect();
        }
        self.blocks
    }
}

/// Rebuilds an 0.44 `Reasoning` from a recorded 0.42 block. Signatures and
/// encrypted payloads survive in `native` so same-source replay can restore
/// the provider's original item; text and redaction map to their fields.
fn convert_legacy_reasoning(block: LegacyReasoning) -> Reasoning {
    // A recorded 0.42 block could hold several content parts; 0.44's shape
    // holds one. The first part decides the mapping and the rest join the
    // visible text, preserving everything providers echo back.
    let mut parts = block.content.into_iter();
    let Some(first) = parts.next() else {
        return Reasoning::default();
    };
    let mut reasoning = match first {
        LegacyReasoningContent::Text { text, signature } => Reasoning {
            text,
            redacted: false,
            native: signature.map(|signature| rig_core::completion::message::Native {
                item: serde_json::json!({ "signature": signature }),
                fingerprint: rig_core::completion::message::Fingerprint::of(&signature),
            }),
        },
        LegacyReasoningContent::Encrypted(data) => Reasoning {
            text: String::new(),
            redacted: false,
            native: Some(rig_core::completion::message::Native {
                item: serde_json::json!({ "encrypted": data }),
                fingerprint: rig_core::completion::message::Fingerprint::of(&data),
            }),
        },
        LegacyReasoningContent::Redacted { data } => Reasoning {
            text: data,
            redacted: true,
            native: None,
        },
        // Summaries folded into the visible text, as 0.44's single `text`
        // field carries summaries too.
        LegacyReasoningContent::Summary(text) => Reasoning::new(text),
    };
    for rest in parts {
        if let LegacyReasoningContent::Text { text, .. } | LegacyReasoningContent::Summary(text) =
            rest
            && !reasoning.redacted
        {
            reasoning.text.push('\n');
            reasoning.text.push_str(&text);
        }
    }
    reasoning
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
        // Earlier blocks count once each; the 0.44 shape holds one block per
        // provider item.
        index
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
                Ok(replay) if replay.source == source => replay
                    .blocks
                    .into_iter()
                    .map(convert_legacy_reasoning)
                    .collect(),
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
            Ok(replay) if replay.source == source => replay.into_blocks(),
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
