//! Record conversation items and attach the producing model's provenance.

use super::session::Session;
use super::turn_context::TurnContext;
use codex_history::ResponseItemEnvelope;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;

impl Session {
    /// Appends to history, persists the prepared items, then notifies raw-item observers.
    /// Execution callers supply their captured model; standalone callers need no tool runtime.
    #[tracing::instrument(level = "trace", skip_all, fields(item_count = items.len()))]
    pub(crate) async fn record_conversation_items(
        &self,
        turn_context: &TurnContext,
        model_info: &ModelInfo,
        items: &[ResponseItem],
    ) {
        self.record_items_with_provenance(turn_context, model_info, items, None)
            .await;
    }

    /// Records model output with the immutable source captured by that request.
    /// Missing source remains unknown; resume never assigns current settings to old output.
    #[tracing::instrument(level = "trace", skip_all, fields(item_count = items.len()))]
    pub(crate) async fn record_model_generated_items(
        &self,
        turn_context: &TurnContext,
        model_info: &ModelInfo,
        items: &[ResponseItem],
        provenance: Option<&codex_history::ModelOutputProvenance>,
    ) {
        self.record_items_with_provenance(turn_context, model_info, items, provenance.cloned())
            .await;
    }

    async fn record_items_with_provenance(
        &self,
        turn_context: &TurnContext,
        model_info: &ModelInfo,
        items: &[ResponseItem],
        provenance: Option<codex_history::ModelOutputProvenance>,
    ) {
        let (items, image_preparations) = self
            .prepare_conversation_items_for_history(turn_context, model_info, items)
            .await;
        let mut items: Vec<ResponseItemEnvelope> = items
            .into_owned()
            .into_iter()
            .map(ResponseItemEnvelope::new)
            .collect();
        if let Some(provenance) = provenance {
            for envelope in &mut items {
                envelope
                    .metadata
                    .get_or_insert_default()
                    .model_output_provenance = Some(provenance.clone());
            }
        }
        self.record_prepared_conversation_items(
            turn_context,
            model_info,
            items,
            image_preparations,
        )
        .await;
    }
}
