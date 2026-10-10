//! Request-only projection of opaque history using captured output ownership.

use codex_api::Provider;
use codex_history::ModelOutputProvenance;
use codex_history::ResponseItemEnvelope;
use codex_login::CodexAuth;
use codex_model_provider_info::ChatBridge;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::models::ResponseItem;

pub(crate) fn request_source(
    info: &ModelProviderInfo,
    provider: &Provider,
    auth: Option<&CodexAuth>,
    model: &str,
    api_auth: &dyn codex_api::AuthProvider,
    agent_identity: Option<&codex_api::AgentIdentityTelemetry>,
    extra_headers: &http::HeaderMap,
) -> Result<ModelOutputProvenance, codex_api::AuthError> {
    let (auth_domain, auth_domain_kind) = auth_domain(
        info,
        provider,
        auth,
        api_auth,
        agent_identity,
        extra_headers,
    )?;
    Ok(ModelOutputProvenance {
        wire_protocol: info.wire_api.to_string(),
        bridge: info.uses_model_bridge().then(|| {
            match info.experimental_bridge {
                Some(ChatBridge::Genai) => "genai",
                Some(ChatBridge::Rig) | Some(ChatBridge::Native) | None => "rig",
            }
            .to_string()
        }),
        provider: info.provider_id.clone(),
        model: Some(model.to_string()),
        endpoint_identity: codex_api::model_endpoint_identity(provider),
        auth_domain,
        auth_domain_kind,
    })
}

fn auth_domain(
    info: &ModelProviderInfo,
    provider: &Provider,
    auth: Option<&CodexAuth>,
    api_auth: &dyn codex_api::AuthProvider,
    agent_identity: Option<&codex_api::AgentIdentityTelemetry>,
    extra_headers: &http::HeaderMap,
) -> Result<(Option<String>, Option<String>), codex_api::AuthError> {
    let snapshot = api_auth.immutable_credential_headers();
    // Every query value — recognized or not — is private scope; the persisted
    // endpoint identity never contains values, so isolation happens here.
    let query_scoped = codex_api::provider_carries_private_query(provider);
    let provider_auth = provider
        .headers
        .keys()
        .any(|name| !codex_api::is_benign_request_header(name.as_str()));
    // Extra headers join the private scope unless the shared policy marks
    // them static telemetry: a future credential-carrying extra header must
    // rotate the identity, while per-attempt trace metadata must not.
    let mut scoped_extra_headers = http::HeaderMap::new();
    for (name, value) in extra_headers {
        if !codex_api::is_benign_request_header(name.as_str()) {
            scoped_extra_headers.append(name.clone(), value.clone());
        }
    }
    let selected_override = query_scoped
        || provider_auth
        || !scoped_extra_headers.is_empty()
        || info.env_key.is_some()
        || info.experimental_bearer_token.is_some()
        || info.auth.is_some()
        || info.gateway_oauth.is_some()
        || info.aws.is_some()
        || info.env_http_headers.as_ref().is_some_and(|headers| {
            headers
                .keys()
                .any(|name| provider.headers.contains_key(name))
        });
    let account = auth.and_then(CodexAuth::get_account_id);
    let user = auth.and_then(CodexAuth::get_chatgpt_user_id);
    let actual_account_matches = snapshot.as_ref().is_some_and(|headers| {
        headers.contains_key(http::header::AUTHORIZATION)
            && headers
                .get("chatgpt-account-id")
                .and_then(|value| value.to_str().ok())
                == account.as_deref()
    }) || agent_identity.is_some();
    // The selected actual account headers (or concrete immutable AgentIdentity
    // owner established by setup) must agree with the captured auth. A global
    // ambient login is never assigned to an anonymous/configured-key request.
    if !selected_override
        && actual_account_matches
        && let (Some(account), Some(user), Some(auth)) = (account, user, auth)
    {
        let identity = serde_json::to_string(&(account, user, auth.get_chatgpt_account_user_id()))
            .map_err(|_| {
                codex_api::AuthError::Build("cannot encode model account identity".into())
            })?;
        return Ok((
            Some(format!("account-v2:{identity}")),
            Some("account".into()),
        ));
    }
    if let Some(headers) = snapshot {
        if headers.is_empty() && !query_scoped && !provider_auth && !selected_override {
            return Ok((Some("anonymous".into()), Some("anonymous".into())));
        }
        if !headers.is_empty() || query_scoped || provider_auth || !scoped_extra_headers.is_empty()
        {
            // Mirror the wire's override order: provider headers, then
            // credential-scoped extra headers, then the auth snapshot.
            let mut actual_headers = provider.headers.clone();
            actual_headers.extend(scoped_extra_headers);
            actual_headers.extend(headers);
            return Ok(
                match codex_api::credential_instance_identity(provider, actual_headers)? {
                    Some(identity) => (Some(identity), Some("credentialInstance".into())),
                    None => (None, None),
                },
            );
        }
    }
    // An unavailable/dynamic snapshot cannot establish credential equality.
    // Configuration selectors may retain unsigned pairs, never signed/opaque data.
    let mut selectors = Vec::new();
    if let Some(name) = &info.env_key {
        selectors.push(("api_key".to_string(), name.clone()));
    }
    if let Some(headers) = &info.env_http_headers {
        selectors.extend(
            headers
                .iter()
                .map(|(name, variable)| (name.to_ascii_lowercase(), variable.clone())),
        );
    }
    selectors.sort();
    if selectors.is_empty() {
        return Ok((None, None));
    }
    let selector = serde_json::to_string(&selectors).map_err(|_| {
        codex_api::AuthError::Build("cannot encode model authentication selector".into())
    })?;
    Ok((
        Some(format!("selector-v1:{selector}")),
        Some("selector".into()),
    ))
}

pub(crate) type InputProvenance =
    std::collections::HashMap<codex_protocol::ResponseItemId, Option<ModelOutputProvenance>>;

pub(crate) fn sources_for_input(input: &[ResponseItemEnvelope]) -> InputProvenance {
    let mut sources = InputProvenance::new();
    for envelope in input {
        if let Some(id) = envelope.item.id() {
            let source = envelope
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.model_output_provenance.clone());
            sources
                .entry(id.clone())
                .and_modify(|previous| {
                    if *previous != source {
                        *previous = None;
                    }
                })
                .or_insert(source);
        }
    }
    sources
}

/// A trusted-runtime authorization to replay exactly one opaque checkpoint
/// item carrying exactly its captured producer provenance.
///
/// The guardian review runtime constructs this after validating the live
/// request against its [`crate::guardian::OpaqueReplayGrant`]; the projection
/// itself only re-checks item identity and producer provenance.
#[derive(Debug, Clone)]
pub(crate) struct OpaqueReplayAuthorization {
    checkpoint_id: codex_protocol::ResponseItemId,
    producer: ModelOutputProvenance,
}

impl OpaqueReplayAuthorization {
    pub(crate) fn new(
        checkpoint_id: codex_protocol::ResponseItemId,
        producer: ModelOutputProvenance,
    ) -> Self {
        Self {
            checkpoint_id,
            producer,
        }
    }

    /// True when `item` is the bound checkpoint and still carries the exact
    /// producer provenance captured when the grant was issued.
    fn preserves(&self, item: &ResponseItem, sources: &InputProvenance) -> bool {
        let Some(id) = item.id() else { return false };
        id == &self.checkpoint_id && sources.get(id) == Some(&Some(self.producer.clone()))
    }
}

/// Removes only incompatible opaque payloads from the outbound copy. Visible
/// reasoning and normal message/tool items survive; stored envelopes never mutate.
/// An [`OpaqueReplayAuthorization`] preserves exactly its bound checkpoint.
pub(crate) fn project_input(
    input: &mut Vec<ResponseItem>,
    sources: &InputProvenance,
    target: &ModelOutputProvenance,
    replay: Option<&OpaqueReplayAuthorization>,
) {
    let target_known = target.provider.is_some()
        && target.model.is_some()
        && target.endpoint_identity.is_some()
        && target.auth_domain.is_some()
        && target.auth_domain_kind.is_some();
    let target_scope_known = target.provider.is_some()
        && target.endpoint_identity.is_some()
        && target.auth_domain.is_some()
        && target.auth_domain_kind.is_some();
    let mut dropped = 0usize;
    input.retain_mut(|item| {
        let compatible = target_known
            && item
                .id()
                .and_then(|id| sources.get(id))
                .and_then(Option::as_ref)
                == Some(target);
        // Compaction checkpoints are backend-validated opaque state, not
        // model-bound ciphertext: replaying them to the same provider,
        // endpoint, wire, and bridge — under a different model of that scope
        // (a model-switch resume) or after a process restart — is legal and
        // the backend rejects a payload it cannot decrypt. Credential-
        // instance domain strings are minted randomly per process, so they
        // are not comparable across that boundary; the evidence KIND must
        // still match. Everything else — reasoning, web-search blocks —
        // stays under the strict comparison.
        let compaction_compatible = target_scope_known
            && item.id().is_some_and(|id| {
                sources.get(id).and_then(Option::as_ref).is_some_and(|source| {
                    source.wire_protocol == target.wire_protocol
                        && source.bridge == target.bridge
                        && source.provider == target.provider
                        && source.endpoint_identity == target.endpoint_identity
                        && source.auth_domain_kind == target.auth_domain_kind
                })
            });
        let trusted_scope_kind = matches!(
            target.auth_domain_kind.as_deref(),
            Some("account" | "anonymous" | "credentialInstance")
        );
        let opaque_compatible = compatible && trusted_scope_kind;
        let compaction_replayable = compaction_compatible && trusted_scope_kind;
        match item {
            ResponseItem::Reasoning {
                encrypted_content,
                summary,
                content,
                ..
            } => {
                if opaque_compatible {
                    return true;
                }
                if encrypted_content.take().is_some() {
                    dropped += 1;
                }
                !summary.is_empty() || content.as_ref().is_some_and(|content| !content.is_empty())
            }
            ResponseItem::Compaction { .. } => {
                let replayed =
                    replay.is_some_and(|authorization| authorization.preserves(item, sources));
                if !(opaque_compatible || compaction_replayable || replayed) {
                    dropped += 1;
                }
                opaque_compatible || compaction_replayable || replayed
            }
            ResponseItem::ContextCompaction {
                encrypted_content, ..
            } => {
                if !opaque_compatible && encrypted_content.take().is_some() {
                    dropped += 1;
                }
                true
            }
            ResponseItem::WebSearchCall { wire_blocks, .. } => {
                if (!compatible
                    || (!opaque_compatible && wire_blocks.as_ref().is_some_and(has_opaque_replay)))
                    && wire_blocks.take().is_some()
                {
                    dropped += 1;
                }
                true
            }
            ResponseItem::Message { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::AdditionalTools { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::FunctionCallOutput { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::CustomToolCallOutput { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::ImageGenerationCall { .. }
            | ResponseItem::ConfigurationUpdate { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::Other => true,
        }
    });
    if dropped > 0 {
        tracing::warn!(model = target.model.as_deref().unwrap_or("unknown"),
            wire = %target.wire_protocol, dropped,
            "Dropped incompatible opaque model history from the outbound request");
    }
}

fn has_opaque_replay(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            object.get("type").and_then(serde_json::Value::as_str) == Some("redacted_thinking")
                || object.iter().any(|(name, value)| {
                    matches!(
                        name.as_str(),
                        "signature"
                            | "encrypted_content"
                            | "encryptedContent"
                            | "encrypted_index"
                            | "encryptedIndex"
                    ) || has_opaque_replay(value)
                })
        }
        serde_json::Value::Array(array) => array.iter().any(has_opaque_replay),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}

#[cfg(test)]
#[path = "model_output_projection_tests.rs"]
mod tests;
