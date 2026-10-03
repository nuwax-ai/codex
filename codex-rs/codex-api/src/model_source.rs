//! Credential-free normalized endpoint ownership for opaque model output.

use crate::Provider;
use sha2::Digest;

/// Identifies the normalized origin/path and routing query, excluding credential
/// query values. The digest is an opaque identity, not an authentication signature.
pub fn model_endpoint_identity(provider: &Provider) -> Option<String> {
    let mut url = url::Url::parse(&provider.base_url).ok()?;
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let mut routing: Vec<_> = url
        .query_pairs()
        .filter(|(name, _)| !credential_query_name(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    if let Some(params) = &provider.query_params {
        let mut configured: Vec<_> = params
            .iter()
            .filter(|(name, _)| !credential_query_name(name))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        configured.sort();
        routing.extend(configured);
    }
    url.set_query(None);
    url.set_fragment(None);
    let encoded = serde_json::to_vec(&(url.as_str().trim_end_matches('/'), routing)).ok()?;
    Some(format!("endpoint-v1:{:x}", sha2::Sha256::digest(encoded)))
}

fn credential_query_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase().replace('-', "_");
    matches!(
        name.as_str(),
        "key"
            | "api_key"
            | "apikey"
            | "access_key"
            | "secret"
            | "credential"
            | "credentials"
            | "token"
            | "access_token"
            | "auth"
            | "authorization"
            | "cookie"
            | "password"
            | "signature"
    ) || name.ends_with("_key")
        || name.ends_with("_credential")
        || name.ends_with("_credentials")
        || name.ends_with("_token")
        || name.ends_with("_secret")
        || name.ends_with("_password")
        || name.ends_with("_signature")
}

#[cfg(test)]
#[path = "model_source_tests.rs"]
mod tests;
