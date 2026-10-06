//! Borrows the latest opaque checkpoint and its recorded producer as one history item.
//! Malformed checkpoints remain visible so consumers cannot fall back to an older grant.

use codex_protocol::models::ResponseItem;

use crate::ResponseItemEnvelope;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompactionCheckpoint<'a> {
    pub item: &'a ResponseItem,
    pub model_hash: Option<&'a str>,
    /// Harness metadata of the checkpoint's persisted envelope, when the
    /// checkpoint came from history. Carries the producing request's
    /// `model_output_provenance`, which replayed checkpoints must keep.
    pub metadata: Option<&'a crate::CodexHarnessMetadata>,
}

impl<'a> CompactionCheckpoint<'a> {
    pub fn latest(items: &'a [ResponseItemEnvelope]) -> Option<Self> {
        items.iter().rev().find_map(|envelope| {
            let mut checkpoint = Self::from_item(
                &envelope.item,
                envelope
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.compaction_model_hash.as_deref()),
            )?;
            checkpoint.metadata = envelope.metadata.as_ref();
            Some(checkpoint)
        })
    }

    /// Missing producer metadata stays unknown, including when the active model changes.
    pub fn from_item(item: &'a ResponseItem, model_hash: Option<&'a str>) -> Option<Self> {
        matches!(
            item,
            ResponseItem::Compaction { .. } | ResponseItem::ContextCompaction { .. }
        )
        .then_some(Self {
            item,
            model_hash,
            metadata: None,
        })
    }

    pub fn is_usable(self) -> bool {
        match self.item {
            ResponseItem::Compaction {
                id: Some(_),
                encrypted_content,
                ..
            }
            | ResponseItem::ContextCompaction {
                id: Some(_),
                encrypted_content: Some(encrypted_content),
                ..
            } => !encrypted_content.is_empty(),
            _ => false,
        }
    }

    pub fn is_compatible_with(self, reviewer_model_hash: Option<&str>) -> bool {
        self.model_hash
            .zip(reviewer_model_hash)
            .is_some_and(|(producer, reviewer)| !producer.is_empty() && producer == reviewer)
    }
}

#[cfg(test)]
#[path = "compaction_checkpoint_tests.rs"]
mod tests;
