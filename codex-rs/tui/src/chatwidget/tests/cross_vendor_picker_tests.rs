//! Regression for the P4 provider wizard: a provider saved mid-session must be
//! visible in `/model` immediately.
//!
//! The picker builds its "other vendors" rows from the widget's own provider
//! snapshot, which normally only changes when the widget is replaced. Saving a
//! provider that is not the active one performs no resume, so without an
//! explicit hand-off the new provider stayed invisible until the next thread
//! switch (reported from a real session: a saved `deepseek` provider produced
//! "Provider `deepseek` saved." but no DeepSeek entries in `/model`).

use super::helpers::make_chatwidget_manual;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_protocol::ThreadId;

fn provider_with_models(name: &str, models: &[&str]) -> ModelProviderInfo {
    ModelProviderInfo {
        name: name.to_string(),
        base_url: Some("https://example.invalid/v1".to_string()),
        wire_api: WireApi::Chat,
        models: Some(models.iter().map(|model| (*model).to_string()).collect()),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_provider_saved_mid_session_appears_in_the_cross_vendor_rows() {
    let (mut chat, _app_event_rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    // Cross-vendor rows are only offered for a conversation that can be
    // re-homed, so the fixture needs a thread.
    chat.thread_id = Some(ThreadId::new());
    let active_provider = chat
        .config
        .model_provider
        .provider_id
        .clone()
        .unwrap_or_default();
    assert_ne!(active_provider, "deepseek", "fixture must not be active");

    let before = chat.cross_vendor_model_items();
    assert!(
        !before.iter().any(|item| item.name == "deepseek-v4-pro"),
        "fixture must start without the provider"
    );

    // What the wizard does after `refresh_in_memory_config_from_disk`.
    let mut providers = chat.config.model_providers.clone();
    providers.insert(
        "deepseek".to_string(),
        provider_with_models("DeepSeek", &["deepseek-v4-pro", "deepseek-flash"]),
    );
    chat.adopt_model_providers(providers);

    let rows = chat.cross_vendor_model_items();
    let row = rows
        .iter()
        .find(|item| item.name == "deepseek-v4-pro")
        .expect("the saved provider's model must be selectable");
    assert_eq!(row.category_tag.as_deref(), Some("DeepSeek"));
    assert!(
        !row.is_disabled,
        "a directly-configured key needs no env var"
    );
    assert!(
        rows.iter().any(|item| item.name == "deepseek-flash"),
        "every declared model must be listed"
    );
}
