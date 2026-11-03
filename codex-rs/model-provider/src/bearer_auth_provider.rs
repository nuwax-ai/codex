use codex_api::AuthProvider;
use codex_http_client::CHATGPT_ACCOUNT_ID_HEADER;
use codex_login::auth::is_fedramp_api_key;
use http::HeaderMap;
use http::HeaderValue;

/// Bearer-token auth provider for OpenAI-compatible model-provider requests.
#[derive(Clone, Default)]
pub struct BearerAuthProvider {
    pub token: Option<String>,
    pub account_id: Option<String>,
    pub is_fedramp_account: bool,
}

#[cfg(test)]
#[path = "bearer_auth_provider_snapshot_tests.rs"]
mod snapshot_tests;

impl BearerAuthProvider {
    pub fn new(token: String) -> Self {
        let is_fedramp_account = is_fedramp_api_key(&token);
        Self {
            token: Some(token),
            account_id: None,
            is_fedramp_account,
        }
    }

    pub fn for_test(token: Option<&str>, account_id: Option<&str>) -> Self {
        Self {
            token: token.map(str::to_string),
            account_id: account_id.map(str::to_string),
            is_fedramp_account: false,
        }
    }
}

impl AuthProvider for BearerAuthProvider {
    // Fork (nuwax-codex): the snapshot is the credential identity anchor for
    // opaque-history projection (`model_output_projection::auth_domain`
    // derives a stable credentialInstance domain from it). Without it the
    // domain falls back to the config-selector form, which never retains
    // compaction checkpoints across turns.
    fn immutable_credential_headers(&self) -> Option<HeaderMap> {
        let mut headers = HeaderMap::new();
        if let Some(token) = &self.token {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}")).ok()?;
            value.set_sensitive(true);
            headers.insert(http::header::AUTHORIZATION, value);
        }
        if let Some(account) = &self.account_id {
            headers.insert(CHATGPT_ACCOUNT_ID_HEADER, HeaderValue::from_str(account).ok()?);
        }
        if self.is_fedramp_account {
            headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
        Some(headers)
    }

    fn resolve_auth_headers(&self) -> codex_api::AuthHeadersFuture<'_> {
        Box::pin(async move {
            self.immutable_credential_headers().ok_or_else(|| {
                codex_api::AuthError::Build("invalid bearer authentication header".into())
            })
        })
    }

    fn add_auth_headers(&self, headers: &mut HeaderMap) -> Result<(), codex_api::AuthError> {
        let resolved = self.immutable_credential_headers().ok_or_else(|| {
            codex_api::AuthError::Build("invalid bearer authentication header".into())
        })?;
        headers.extend(resolved);
        Ok(())
    }
}

#[cfg(test)]
#[path = "bearer_auth_provider_tests.rs"]
mod tests;
