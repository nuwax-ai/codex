//! Persist complete provider reasoning blocks, including signatures and redactions.
//! The versioned envelope is opaque to Codex and is only replayed to its original
//! endpoint, protocol and model. Native encrypted Responses payloads are never text.

use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ResponseItem;
use rig_core::completion::message::Reasoning;
use rig_core::completion::message::ReasoningContent;

pub(crate) const REPLAY_PREFIX: &str = "codex-rig-reasoning-v1:";

#[derive(serde::Serialize, serde::Deserialize)]
struct Replay {
    source: String,
    blocks: Vec<Reasoning>,
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
        if let Some(ReasoningContent::Text {
            text: accumulated, ..
        }) = self.blocks[index].content.first_mut()
        {
            accumulated.push_str(text);
        }
        self.blocks[..index]
            .iter()
            .flat_map(|block| &block.content)
            .filter(|part| matches!(part, ReasoningContent::Text { .. }))
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
        let content = blocks
            .iter()
            .flat_map(|reasoning| reasoning.content.iter())
            .filter_map(|block| match block {
                ReasoningContent::Text { text, .. } => {
                    Some(ReasoningItemContent::ReasoningText { text: text.clone() })
                }
                ReasoningContent::Summary(_)
                | ReasoningContent::Encrypted(_)
                | ReasoningContent::Redacted { .. } => None,
            })
            .collect();
        // Only strings and Vecs are serialized here; no non-string map keys or floats.
        match serde_json::to_string(&Replay {
            source: source.to_string(),
            blocks,
        }) {
            Ok(encoded) => Some((content, format!("{REPLAY_PREFIX}{encoded}"))),
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
    if let Some(encoded) = encrypted_content
        .as_deref()
        .and_then(|value| value.strip_prefix(REPLAY_PREFIX))
    {
        return match serde_json::from_str::<Replay>(encoded) {
            Ok(replay) if replay.source == source => replay.blocks,
            Ok(_) => Vec::new(),
            Err(error) => {
                tracing::warn!(%error, "Ignoring invalid Rig reasoning replay envelope");
                Vec::new()
            }
        };
    }
    // Legacy bridge histories duplicated plain text into both fields. Accept
    // only that exact shape; opaque OpenAI encrypted reasoning is never echoed.
    let text: String = content
        .iter()
        .flatten()
        .map(|part| match part {
            ReasoningItemContent::ReasoningText { text } | ReasoningItemContent::Text { text } => {
                text.as_str()
            }
        })
        .collect();
    if protocol == crate::RigProtocol::Chat
        && !text.is_empty()
        && encrypted_content.as_deref() == Some(text.as_str())
    {
        vec![Reasoning::new(&text)]
    } else {
        Vec::new()
    }
}
