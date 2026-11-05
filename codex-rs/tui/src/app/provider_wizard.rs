//! Fork (nuwax-codex): App-side step machine for the add-a-model-provider
//! wizard (FORK.md §7.3). The ChatWidget renders each step (see
//! `chatwidget/provider_wizard.rs`); these handlers validate the input,
//! advance the draft, and write the finished provider via config/batchWrite.
//! One AppEvent variant per step; the dispatch arms call straight through.

use super::*;
use crate::chatwidget::WizardCredentialIntent;

impl App {
    pub(super) fn provider_wizard_open(&mut self) {
        self.provider_wizard = Some(Default::default());
        // A stale intent from an aborted run must not leak a previously
        // pasted key into this run's write.
        self.wizard_credential = None;
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
                self.chat_widget.show_provider_wizard_credential_step();
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
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !valid {
            self.chat_widget.add_error_message(
                "Provider id must be lowercase letters, digits, and dashes.".to_string(),
            );
            self.show_wizard_id_step(value.to_string());
            return;
        }
        // Reserved ids fail at the engine's config validation; reject them
        // here so the user learns at step one, not after six steps.
        let reserved = [
            "openai",
            "oss",
            "lmstudio",
            "gcp",
            "amazon-bedrock",
            "amazon-bedrock-runtime",
            "nuwax_env",
        ];
        if reserved.contains(&id.as_str()) {
            self.chat_widget.add_error_message(format!(
                "`{id}` is a reserved provider id — pick another key."
            ));
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
        if let Some(draft) = self.provider_wizard.as_mut() {
            draft.wire_api = wire.to_string();
        }
        self.chat_widget.show_provider_wizard_credential_step();
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
            draft.env_key = env_key.clone();
            draft.models.join(", ")
        };
        self.wizard_credential = Some(if env_key.is_empty() {
            WizardCredentialIntent::NoAuth
        } else {
            WizardCredentialIntent::EnvKey(env_key)
        });
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
            self.show_wizard_models_step(value.to_string());
            return;
        }
        draft.models = models;
        let Some(draft) = self.provider_wizard.as_ref() else {
            return;
        };
        self.chat_widget.show_provider_wizard_preview(
            draft,
            &self.wizard_credential_note(),
            &self.wizard_after_save_note(),
        );
    }

    /// The after-save hint, matching the credential mode.
    fn wizard_after_save_note(&self) -> String {
        match &self.wizard_credential {
            Some(WizardCredentialIntent::DirectKey(_)) => {
                "After saving: the key is stored in your user config (0600) — no export \
                 needed; choose the provider's models with /model."
                    .to_string()
            }
            Some(WizardCredentialIntent::EnvKey(name)) => {
                format!(
                    "After saving: export {name}=<your key> (the key itself never goes \
                     into config), then choose the provider's models with /model."
                )
            }
            Some(WizardCredentialIntent::NoAuth) => {
                "After saving: choose the provider's models with /model.".to_string()
            }
            None => "After saving: choose the provider's models with /model.".to_string(),
        }
    }

    /// The preview line for the credential slot — a status, never a value.
    fn wizard_credential_note(&self) -> String {
        match &self.wizard_credential {
            Some(WizardCredentialIntent::DirectKey(_)) => {
                "api_key = \u{2022}\u{2022}\u{2022}\u{2022} (will be saved from your input)"
                    .to_string()
            }
            Some(WizardCredentialIntent::EnvKey(name)) => {
                format!("env_key = {name:?}")
            }
            Some(WizardCredentialIntent::NoAuth) => {
                "# no authentication (local gateway)".to_string()
            }
            None => "env_key = \"\"".to_string(),
        }
    }

    pub(super) async fn provider_wizard_confirm(&mut self, app_server: &mut AppServerSession) {
        // Keep the draft until the write succeeds so a failed save can be
        // retried from the preview instead of redoing every step.
        let Some(draft) = self.provider_wizard.clone() else {
            return;
        };
        let provider_id = draft.provider_id.clone();
        let env_key = draft.env_key.clone();
        let edits = match &self.wizard_credential {
            Some(WizardCredentialIntent::DirectKey(key)) => draft.config_edits_with_api_key(key),
            _ => draft.config_edits(),
        };
        match self
            .persist_model_defaults(app_server.request_handle(), edits, "model provider")
            .await
        {
            Ok(()) => {
                self.provider_wizard = None;
                self.wizard_credential = None;
                // Refresh the in-memory config so the new provider is visible
                // to /model in this same session (the preview footer promises
                // exactly that).
                self.refresh_in_memory_config_from_disk_best_effort("saving a model provider")
                    .await;
                let key_hint = if env_key.is_empty() {
                    String::new()
                } else {
                    format!(" Set ${env_key} to use it.")
                };
                self.chat_widget.add_info_message(
                    format!("Provider `{provider_id}` saved.{key_hint}"),
                    /*hint*/ None,
                );
            }
            Err(err) => {
                let error = crate::config_update::format_config_error(&err);
                self.chat_widget.add_error_message(format!(
                    "Failed to save provider `{provider_id}`: {error} — adjust the draft or \
                     confirm again to retry."
                ));
                if let Some(draft) = self.provider_wizard.as_ref() {
                    self.chat_widget.show_provider_wizard_preview(
                        draft,
                        &self.wizard_credential_note(),
                        &self.wizard_after_save_note(),
                    );
                }
            }
        }
    }

    fn show_wizard_models_step(&mut self, initial: String) {
        self.chat_widget.show_provider_wizard_text_step(
            "Models",
            "Comma-separated model slugs this provider serves — these are what /model \
             lists. Metadata (context window, effort tiers) resolves from the bundled \
             vendor catalog per slug.",
            initial,
            AppEvent::ProviderWizardModels,
        );
    }

    fn show_wizard_id_step(&mut self, initial: String) {
        self.chat_widget.show_provider_wizard_text_step(
            "Provider id",
            "The config.toml key, e.g. `my-gateway`",
            initial,
            AppEvent::ProviderWizardId,
        );
    }

    /// Fork: route the credential-mode choice. Direct key opens the masked
    /// input; env var opens the variable-name step; no auth skips ahead.
    pub(super) fn provider_wizard_credential_mode(&mut self, mode: &str) {
        match mode {
            "API key (paste)" => self.chat_widget.show_provider_wizard_key_input(),
            "Environment variable (advanced)" => {
                let initial = self
                    .provider_wizard
                    .as_ref()
                    .map(|draft| draft.env_key.clone())
                    .unwrap_or_default();
                self.show_wizard_env_key_step(initial);
            }
            "No authentication" => {
                if let Some(draft) = self.provider_wizard.as_mut() {
                    draft.env_key = String::new();
                }
                self.wizard_credential = Some(WizardCredentialIntent::NoAuth);
                let models = self
                    .provider_wizard
                    .as_ref()
                    .map(|draft| draft.models.join(", "))
                    .unwrap_or_default();
                self.show_wizard_models_step(models);
            }
            _ => {}
        }
    }

    /// Fork: store the pasted key and advance to the models step. The key
    /// lives only in the draft; the TOML preview shows "will be saved as
    /// api_key", never the value.
    pub(super) fn provider_wizard_api_key(&mut self, key: &str) {
        let key = key.trim().to_string();
        if key.is_empty() {
            self.chat_widget.add_error_message(
                "The API key must not be empty — paste a key or pick another \
                 authentication mode."
                    .to_string(),
            );
            self.chat_widget.show_provider_wizard_credential_step();
            return;
        }
        if let Some(draft) = self.provider_wizard.as_mut() {
            draft.env_key = String::new();
        }
        self.wizard_credential = Some(WizardCredentialIntent::DirectKey(key));
        let models = self
            .provider_wizard
            .as_ref()
            .map(|draft| draft.models.join(", "))
            .unwrap_or_default();
        self.show_wizard_models_step(models);
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
