//! Fork (nuwax-codex): App-side step machine for the add-a-model-provider
//! wizard (FORK.md §7.3). The ChatWidget renders each step (see
//! `chatwidget/provider_wizard.rs`); these handlers validate the input,
//! advance the draft, and write the finished provider via config/batchWrite.
//! One AppEvent variant per step; the dispatch arms call straight through.

use super::*;

impl App {
    pub(super) fn provider_wizard_open(&mut self) {
        self.provider_wizard = Some(Default::default());
        self.chat_widget.open_provider_wizard();
    }

    pub(super) fn provider_wizard_preset(&mut self, id: &str) {
        match crate::chatwidget::PROVIDER_PRESETS
            .iter()
            .find(|preset| preset.id == id)
        {
            Some(preset) => {
                self.provider_wizard =
                    Some(crate::chatwidget::ProviderWizardDraft::from_preset(preset));
                let initial = self
                    .provider_wizard
                    .as_ref()
                    .map(|draft| draft.env_key.clone())
                    .unwrap_or_default();
                self.show_wizard_env_key_step(initial);
            }
            // Custom: collect everything by hand, starting with the id.
            None => {
                self.provider_wizard = Some(Default::default());
                self.show_wizard_id_step(String::new());
            }
        }
    }

    pub(super) fn provider_wizard_id(&mut self, value: &str) {
        let id = value.trim().to_ascii_lowercase();
        let valid = !id.is_empty()
            && id.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            self.chat_widget.add_error_message(
                "Provider id must be lowercase letters, digits, and dashes.".to_string(),
            );
            self.show_wizard_id_step(value.to_string());
            return;
        }
        if id == "nuwax_env" {
            self.chat_widget
                .add_error_message("`nuwax_env` is a reserved provider id.".to_string());
            self.show_wizard_id_step(value.to_string());
            return;
        }
        if let Some(draft) = self.provider_wizard.as_mut() {
            draft.provider_id = id;
        }
        self.chat_widget.show_provider_wizard_text_step(
            "Display name",
            "Shown in pickers and status lines (defaults to the id)",
            String::new(),
            AppEvent::ProviderWizardName,
        );
    }

    pub(super) fn provider_wizard_name(&mut self, value: &str) {
        let Some(draft) = self.provider_wizard.as_mut() else {
            return;
        };
        let name = value.trim().to_string();
        draft.name = if name.is_empty() {
            draft.provider_id.clone()
        } else {
            name
        };
        self.chat_widget.show_provider_wizard_text_step(
            "Base URL",
            "The vendor's API base, e.g. https://api.example.com/v1 — no path suffixes",
            String::new(),
            AppEvent::ProviderWizardBaseUrl,
        );
    }

    pub(super) fn provider_wizard_base_url(&mut self, value: &str) {
        let url = value.trim().to_string();
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.chat_widget
                .add_error_message("Base URL must start with http:// or https://.".to_string());
            self.chat_widget.show_provider_wizard_text_step(
                "Base URL",
                "The vendor's API base, e.g. https://api.example.com/v1 — no path suffixes",
                value.to_string(),
                AppEvent::ProviderWizardBaseUrl,
            );
            return;
        }
        if let Some(draft) = self.provider_wizard.as_mut() {
            draft.base_url = url;
        }
        self.chat_widget.show_provider_wizard_wire_step();
    }

    pub(super) fn provider_wizard_wire(&mut self, wire: &str) {
        if !matches!(wire, "responses" | "chat" | "anthropic") {
            return;
        }
        let initial = self
            .provider_wizard
            .as_ref()
            .map(|draft| draft.env_key.clone())
            .unwrap_or_default();
        if let Some(draft) = self.provider_wizard.as_mut() {
            draft.wire_api = wire.to_string();
        }
        self.show_wizard_env_key_step(initial);
    }

    pub(super) fn provider_wizard_env_key(&mut self, value: &str) {
        let env_key = value.trim().to_string();
        if !env_key.is_empty()
            && !env_key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            self.chat_widget.add_error_message(
                "env_key must be UPPER_SNAKE_CASE (or empty for a local gateway).".to_string(),
            );
            self.show_wizard_env_key_step(env_key);
            return;
        }
        let initial = {
            let Some(draft) = self.provider_wizard.as_mut() else {
                return;
            };
            draft.env_key = env_key;
            draft.models.join(", ")
        };
        self.chat_widget.show_provider_wizard_text_step(
            "Models",
            "Comma-separated model slugs this provider serves — these are what /model \
             lists. Metadata (context window, effort tiers) resolves from the bundled \
             vendor catalog per slug.",
            initial,
            AppEvent::ProviderWizardModels,
        );
    }

    pub(super) fn provider_wizard_models(&mut self, value: &str) {
        let models: Vec<String> = value
            .split([',', '\n'])
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .map(str::to_string)
            .collect();
        let Some(draft) = self.provider_wizard.as_mut() else {
            return;
        };
        if models.is_empty() {
            self.chat_widget.add_error_message(
                "List at least one model slug — an empty list would leave /model with \
                 nothing to offer."
                    .to_string(),
            );
            return;
        }
        draft.models = models;
        let Some(draft) = self.provider_wizard.as_ref() else {
            return;
        };
        self.chat_widget.show_provider_wizard_preview(draft);
    }

    pub(super) async fn provider_wizard_confirm(&mut self, app_server: &mut AppServerSession) {
        let Some(draft) = self.provider_wizard.take() else {
            return;
        };
        let provider_id = draft.provider_id.clone();
        let env_key = draft.env_key.clone();
        match self
            .persist_model_defaults(app_server.request_handle(), draft.config_edits(), "model provider")
            .await
        {
            Ok(()) => {
                let key_hint = if env_key.is_empty() {
                    String::new()
                } else {
                    format!(" Set ${env_key} and restart the TUI to use it.")
                };
                self.chat_widget.add_info_message(
                    format!("Provider `{provider_id}` saved.{key_hint}"),
                    /*hint*/ None,
                );
            }
            Err(err) => {
                let error = crate::config_update::format_config_error(&err);
                self.chat_widget.add_error_message(format!(
                    "Failed to save provider `{provider_id}`: {error}"
                ));
            }
        }
    }

    fn show_wizard_id_step(&mut self, initial: String) {
        self.chat_widget.show_provider_wizard_text_step(
            "Provider id",
            "The config.toml key, e.g. `my-gateway`",
            initial,
            AppEvent::ProviderWizardId,
        );
    }

    fn show_wizard_env_key_step(&mut self, initial: String) {
        self.chat_widget.show_provider_wizard_text_step(
            "API key environment variable (env_key)",
            "The NAME of the variable holding your key — the key itself never goes \
             into config. Leave empty for a local gateway without auth.",
            initial,
            AppEvent::ProviderWizardEnvKey,
        );
    }
}
