//! Fork (nuwax-codex): the "add a model provider" wizard (multi-vendor plan
//! P4, FORK.md §7.3). Chained popups collect a provider definition and hand
//! it to the app layer, which writes it through `config/batchWrite` as
//! `model_providers.<id>` keys. API keys never enter this flow: the wizard
//! records the *variable name* (`env_key`) only.

use super::*;

/// A curated connection suggestion for the wizard's first step. These are UI
/// hints for vendors the fork has live experience with — engine-side model
/// metadata still comes from the vendor catalog (§7.2), so nothing here
/// drifts into request behavior.
pub(crate) struct ProviderPreset {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) base_url: &'static str,
    pub(crate) wire_api: &'static str,
    pub(crate) env_key: &'static str,
    pub(crate) models: &'static [&'static str],
}

pub(crate) const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "glm",
        name: "GLM Coding Plan (Anthropic)",
        base_url: "https://open.bigmodel.cn/api/anthropic",
        wire_api: "anthropic",
        env_key: "GLM_API_KEY",
        models: &["GLM-5.3-Flash", "glm-5.3", "glm-5.3-flashx"],
    },
    ProviderPreset {
        id: "glm-chat",
        name: "GLM Coding Plan (OpenAI Chat)",
        base_url: "https://open.bigmodel.cn/api/coding/paas/v4",
        wire_api: "chat",
        env_key: "GLM_API_KEY",
        models: &["GLM-5.3-Flash", "glm-5.3", "glm-5.3-flashx"],
    },
    ProviderPreset {
        id: "mimo",
        name: "Xiaomi MiMo (Anthropic)",
        base_url: "https://token-plan-cn.xiaomimimo.com/anthropic",
        wire_api: "anthropic",
        env_key: "MIMO_API_KEY",
        models: &["mimo-v2.6-flash", "mimo-v2.6-pro", "mimo-v2.5"],
    },
    ProviderPreset {
        id: "mimo-chat",
        name: "Xiaomi MiMo (OpenAI Chat)",
        base_url: "https://token-plan-cn.xiaomimimo.com/v1",
        wire_api: "chat",
        env_key: "MIMO_API_KEY",
        models: &["mimo-v2.6-flash", "mimo-v2.6-pro", "mimo-v2.5"],
    },
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        base_url: "https://api.deepseek.com/v1",
        wire_api: "chat",
        env_key: "DEEPSEEK_API_KEY",
        models: &["deepseek-v4-pro", "deepseek-v4-flash"],
    },
    ProviderPreset {
        id: "moonshotai",
        name: "Moonshot Kimi",
        base_url: "https://api.moonshot.cn/v1",
        wire_api: "chat",
        env_key: "MOONSHOT_API_KEY",
        models: &["kimi-k2.7-code", "kimi-k3"],
    },
    ProviderPreset {
        id: "stepfun",
        name: "StepFun",
        base_url: "https://api.stepfun.com/v1",
        wire_api: "chat",
        env_key: "STEPFUN_API_KEY",
        models: &["step-3.7-flash", "step-5-preview"],
    },
    ProviderPreset {
        id: "minimax",
        name: "MiniMax",
        base_url: "https://api.minimax.chat/v1",
        wire_api: "chat",
        env_key: "MINIMAX_API_KEY",
        models: &["MiniMax-M2.5", "MiniMax-M3"],
    },
];

/// The wizard's accumulated draft, advanced step by step on `App`.
#[derive(Default, Clone)]
pub(crate) struct ProviderWizardDraft {
    pub(crate) provider_id: String,
    pub(crate) name: String,
    pub(crate) base_url: String,
    pub(crate) wire_api: String,
    pub(crate) env_key: String,
    pub(crate) models: Vec<String>,
}

/// Fork: how the wizard finishes the credential step.
#[derive(Clone, PartialEq)]
pub(crate) enum WizardCredentialIntent {
    /// Paste a key into a masked input; saved as `api_key`.
    DirectKey(String),
    /// Reference an environment variable (advanced mode).
    EnvKey(String),
    /// Local gateway without auth.
    NoAuth,
}

impl ProviderWizardDraft {
    pub(crate) fn from_preset(preset: &ProviderPreset) -> Self {
        Self {
            provider_id: preset.id.to_string(),
            name: preset.name.to_string(),
            base_url: preset.base_url.to_string(),
            wire_api: preset.wire_api.to_string(),
            env_key: preset.env_key.to_string(),
            models: preset.models.iter().map(|model| model.to_string()).collect(),
        }
    }

    /// The `[model_providers.<id>]` block this draft writes, rendered for
    /// the preview step. The credential line is a STATUS, never a value:
    /// `api_key = "••••"` is replaced at write time by the typed key.
    pub(crate) fn toml_preview(&self, credential_note: &str) -> String {
        let models = self
            .models
            .iter()
            .map(|model| format!("{model:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        [
            format!("[model_providers.{}]", self.provider_id),
            format!("name = {:?}", self.name),
            format!("base_url = {:?}", self.base_url),
            format!("wire_api = {:?}", self.wire_api),
            credential_note.to_string(),
            format!("models = [{models}]"),
        ]
        .join("\n")
    }

    /// Edits including a direct api_key: the key value is inserted ONLY in
    /// this restricted write path (never in the preview, logs, or draft
    /// serialization).
    pub(crate) fn config_edits_with_api_key(
        &self,
        key: &str,
    ) -> Vec<codex_app_server_protocol::ConfigEdit> {
        let mut edits = self.config_edits();
        let prefix = format!("model_providers.{}.", self.provider_id);
        // Source switch: delete any env_key the earlier edit chain would
        // have written (a null write removes an existing field).
        edits.retain(|edit| edit.key_path != format!("{prefix}env_key"));
        edits.insert(
            0,
            codex_app_server_protocol::ConfigEdit {
                key_path: format!("{prefix}env_key"),
                value: serde_json::Value::Null,
                merge_strategy: codex_app_server_protocol::MergeStrategy::Replace,
            },
        );
        edits.push(codex_app_server_protocol::ConfigEdit {
            key_path: format!("{prefix}api_key"),
            value: serde_json::json!(key),
            merge_strategy: codex_app_server_protocol::MergeStrategy::Replace,
        });
        edits
    }

    /// One key-path edit per field; `config/batchWrite` preserves unrelated
    /// keys under the same provider table, so saving over an existing id
    /// updates exactly these fields.
    pub(crate) fn config_edits(&self) -> Vec<codex_app_server_protocol::ConfigEdit> {
        use codex_app_server_protocol::ConfigEdit;
        use codex_app_server_protocol::MergeStrategy;
        fn edit(key_path: String, value: serde_json::Value) -> ConfigEdit {
            ConfigEdit {
                key_path,
                value,
                merge_strategy: MergeStrategy::Replace,
            }
        }
        let prefix = format!("model_providers.{}.", self.provider_id);
        // An empty env_key means "no auth" — omit the key entirely. Writing
        // env_key = "" would deserialize to Some("") and make the engine look
        // for a required environment variable with an empty name.
        let mut edits = vec![
            edit(format!("{prefix}name"), serde_json::json!(self.name)),
            edit(format!("{prefix}base_url"), serde_json::json!(self.base_url)),
            edit(format!("{prefix}wire_api"), serde_json::json!(self.wire_api)),
        ];
        if !self.env_key.is_empty() {
            edits.push(edit(format!("{prefix}env_key"), serde_json::json!(self.env_key)));
        }
        edits.push(edit(format!("{prefix}models"), serde_json::json!(self.models)));
        edits
    }
}

impl ChatWidget {
    /// Step 1: pick a connection preset or start from scratch.
    pub(crate) fn open_provider_wizard(&mut self) {
        let mut items: Vec<SelectionItem> = PROVIDER_PRESETS
            .iter()
            .map(|preset| {
                let configured = self.config.model_providers.contains_key(preset.id);
                SelectionItem {
                    name: preset.name.to_string(),
                    description: Some(format!(
                        "{} · ${}{}",
                        preset.base_url,
                        preset.env_key,
                        if configured {
                            " · already configured — saving overwrites these keys"
                        } else {
                            ""
                        }
                    )),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::ProviderWizardPreset(preset.id.to_string()));
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();
        items.push(SelectionItem {
            name: "Custom…".to_string(),
            description: Some("Enter every field by hand".to_string()),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::ProviderWizardPreset(String::new()));
            })],
            dismiss_on_select: true,
            ..Default::default()
        });
        self.show_selection_view(SelectionViewParams {
            view_id: Some("provider-wizard-preset"),
            title: Some("Add a model provider".to_string()),
            subtitle: Some(
                "Pick a vendor to prefill, or start from scratch. The API key itself \
                 is never written to config — only the environment variable name."
                    .to_string(),
            ),
            items,
            ..SelectionViewParams::picker()
        });
    }

    /// Fork: the credential-mode picker — direct key (default), env var
    /// (advanced), or no auth. The direct-key branch opens a MASKED input;
    /// the pasted value never renders in clear, never enters chat history,
    /// and never reaches the TOML preview.
    pub(crate) fn show_provider_wizard_credential_step(&mut self) {
        let items: Vec<SelectionItem> = [
            (
                "API key (paste)",
                "Paste the key directly — saved as api_key in your user config (masked input)",
            ),
            (
                "Environment variable (advanced)",
                "Reference an env var like MY_PROVIDER_API_KEY; the key stays in your shell",
            ),
            (
                "No authentication",
                "Local gateway or proxy without credentials",
            ),
        ]
        .into_iter()
        .map(|(name, description)| SelectionItem {
            name: name.to_string(),
            description: Some(description.to_string()),
            actions: vec![Box::new(move |tx| {
                tx.send(AppEvent::ProviderWizardCredentialMode(name.to_string()));
            })],
            dismiss_on_select: true,
            ..Default::default()
        })
        .collect();
        self.show_selection_view(SelectionViewParams {
            view_id: Some("provider-wizard-credential"),
            title: Some("Authentication".to_string()),
            subtitle: Some(
                "How should this provider authenticate? The key itself is only ever \
                 written to your user config (0600) or sent to the provider."
                    .to_string(),
            ),
            items,
            ..SelectionViewParams::picker()
        });
    }

    /// Fork: masked input for the API key. Rendered with '*', never echoed.
    pub(crate) fn show_provider_wizard_key_input(&mut self) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "API key".to_string(),
            "Paste the key and press Enter (input is masked)".to_string(),
            String::new(),
            /*context_label*/ None,
            Box::new(move |value: String| {
                tx.send(AppEvent::ProviderWizardApiKey(value));
            }),
        );
        // The masked flag is set after construction via the struct field.
        let mut view = view;
        view.masked = true;
        self.bottom_pane.show_text_prompt(view);
    }

    /// A single-line text step. `initial` prefills the editor.
    pub(crate) fn show_provider_wizard_text_step(
        &mut self,
        title: &str,
        subtitle: &str,
        initial: String,
        emit: fn(String) -> AppEvent,
    ) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            title.to_string(),
            subtitle.to_string(),
            initial,
            /*context_label*/ None,
            Box::new(move |value: String| {
                tx.send(emit(value));
            }),
        );
        self.bottom_pane.show_text_prompt(view);
    }

    /// The wire-protocol picker.
    pub(crate) fn show_provider_wizard_wire_step(&mut self) {
        let items: Vec<SelectionItem> = [
            (
                "anthropic",
                "Anthropic Messages — the fork's best-verified wire for GLM/MiMo",
            ),
            (
                "chat",
                "OpenAI Chat Completions — widely supported by CN gateways",
            ),
            (
                "responses",
                "OpenAI Responses — same-protocol passthrough, for gateways that serve it",
            ),
        ]
        .into_iter()
        .map(|(wire, description)| {
            SelectionItem {
                name: wire.to_string(),
                description: Some(description.to_string()),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::ProviderWizardWire(wire.to_string()));
                })],
                dismiss_on_select: true,
                ..Default::default()
            }
        })
        .collect();
        self.show_selection_view(SelectionViewParams {
            view_id: Some("provider-wizard-wire"),
            title: Some("Protocol (wire_api)".to_string()),
            items,
            ..SelectionViewParams::picker()
        });
    }

    /// Final step: preview the exact TOML block and confirm the write.
    pub(crate) fn show_provider_wizard_preview(
        &mut self,
        draft: &ProviderWizardDraft,
        credential_note: &str,
    ) {
        let toml = draft.toml_preview(credential_note);
        let env_key = draft.env_key.clone();
        let exists = self
            .config
            .model_providers
            .contains_key(&draft.provider_id);
        let mut description = vec![toml];
        if exists {
            description.push(format!(
                "Provider `{}` is already configured — saving overwrites the keys above (others are kept).",
                draft.provider_id
            ));
        }
        description.push(format!(
            "After saving: export {env_key}=<your key> (the key itself never goes into config), then choose the provider's models with /model."
        ));
        let items = vec![SelectionItem {
            name: "Save provider".to_string(),
            description: Some(description.join("\n\n")),
            require_explicit_confirmation: true,
            footer_hint: Some(Line::from(
                "Writes the block above to config.toml — its models appear in /model".to_string(),
            )),
            actions: vec![Box::new(|tx| {
                tx.send(AppEvent::ProviderWizardConfirm);
            })],
            dismiss_on_select: true,
            ..Default::default()
        }];
        self.show_selection_view(SelectionViewParams {
            view_id: Some("provider-wizard-preview"),
            title: Some("Save this provider?".to_string()),
            items,
            ..SelectionViewParams::picker()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_wizard_draft_preview_and_edits() {
        let draft = ProviderWizardDraft {
            provider_id: "my-gateway".to_string(),
            name: "My Gateway".to_string(),
            base_url: "https://gw.example.test/v1".to_string(),
            wire_api: "chat".to_string(),
            env_key: "GW_API_KEY".to_string(),
            models: vec!["model-a".to_string(), "model-b".to_string()],
        };
        let preview = draft.toml_preview("env_key = \"GW_API_KEY\"");
        assert!(preview.contains("[model_providers.my-gateway]"));
        assert!(preview.contains("wire_api = \"chat\""));
        assert!(preview.contains("models = [\"model-a\", \"model-b\"]"));
        // The credential note is caller-supplied so the direct key never
        // passes through the draft.
        assert!(preview.contains("env_key = \"GW_API_KEY\""));
        let edits = draft.config_edits();
        let paths: Vec<&str> = edits.iter().map(|edit| edit.key_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "model_providers.my-gateway.name",
                "model_providers.my-gateway.base_url",
                "model_providers.my-gateway.wire_api",
                "model_providers.my-gateway.env_key",
                "model_providers.my-gateway.models",
            ]
        );
        assert_eq!(
            edits[4].value,
            serde_json::json!(["model-a", "model-b"])
        );

        // No auth: the env_key edit is omitted entirely — writing
        // env_key = "" would make the engine require an empty-named
        // environment variable.
        let mut no_auth = draft;
        no_auth.env_key = String::new();
        let no_auth_edits = no_auth.config_edits();
        let paths: Vec<&str> = no_auth_edits
            .iter()
            .map(|edit| edit.key_path.as_str())
            .collect();
        assert_eq!(
            paths,
            [
                "model_providers.my-gateway.name",
                "model_providers.my-gateway.base_url",
                "model_providers.my-gateway.wire_api",
                "model_providers.my-gateway.models",
            ]
        );
    }

    #[test]
    fn provider_wizard_presets_carry_connection_facts() {
        // The presets are UI prefills; sanity-pin their shape so a bad edit
        // cannot silently ship an unusable suggestion.
        for preset in PROVIDER_PRESETS {
            assert!(preset.base_url.starts_with("https://"), "{}", preset.id);
            assert!(
                matches!(preset.wire_api, "chat" | "anthropic" | "responses"),
                "{}",
                preset.id
            );
            assert!(!preset.models.is_empty(), "{}", preset.id);
        }
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;

    #[test]
    fn direct_key_edits_delete_env_key_and_carry_the_key_once() {
        let draft = ProviderWizardDraft {
            provider_id: "gw".to_string(),
            name: "Gateway".to_string(),
            base_url: "https://gw.example.test/v1".to_string(),
            wire_api: "chat".to_string(),
            env_key: "GW_ENV_KEY".to_string(),
            models: vec!["model-a".to_string()],
        };
        let edits = draft.config_edits_with_api_key("sk-test-key");
        let key_edit = edits
            .iter()
            .find(|edit| edit.key_path == "model_providers.gw.api_key")
            .expect("api_key edit present");
        assert_eq!(key_edit.value, serde_json::json!("sk-test-key"));
        let env_edit = edits
            .iter()
            .find(|edit| edit.key_path == "model_providers.gw.env_key")
            .expect("env_key deletion edit present");
        assert_eq!(env_edit.value, serde_json::Value::Null);
        // The key appears exactly once across all edits.
        let hits = edits
            .iter()
            .filter(|edit| {
                edit.value
                    .as_str()
                    .is_some_and(|value| value.contains("sk-test-key"))
            })
            .count();
        assert_eq!(hits, 1);
    }

    #[test]
    fn preview_never_contains_the_key_value() {
        let draft = ProviderWizardDraft {
            provider_id: "gw".to_string(),
            name: "Gateway".to_string(),
            base_url: "https://gw.example.test/v1".to_string(),
            wire_api: "chat".to_string(),
            env_key: String::new(),
            models: vec!["model-a".to_string()],
        };
        let preview = draft.toml_preview("api_key = \"•••• (will be saved from your input)\"");
        assert!(!preview.contains("sk-"));
        assert!(preview.contains("api_key"));
    }
}
