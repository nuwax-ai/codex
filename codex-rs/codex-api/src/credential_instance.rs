//! Bounded process-local credential equality. Secrets are neither hashed,
//! serialized nor exposed through Debug. Only a fresh random identity escapes.

use crate::AuthError;
use crate::Provider;
use http::HeaderMap;
use rand::TryRngCore;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::OnceLock;

const MAX_ENTRIES: usize = 64;
const MAX_ENTRY_BYTES: usize = 16 * 1024;

struct Entry {
    headers: HeaderMap,
    base_url: String,
    /// Exact wire query: URL-embedded pairs in URL order, then sorted config
    /// pairs — the same shape the transport sends. Order and duplicates are
    /// preserved so two different query strings never compare equal.
    query: Vec<(String, String)>,
    id: String,
}

#[derive(Default)]
struct InstanceCache {
    entries: VecDeque<Entry>,
}

impl InstanceCache {
    fn identify(
        &mut self,
        provider: &Provider,
        headers: HeaderMap,
        random_id: impl FnOnce() -> Result<String, AuthError>,
    ) -> Result<Option<String>, AuthError> {
        let bytes = headers
            .iter()
            .fold(provider.base_url.len(), |total, (name, value)| {
                total
                    .saturating_add(name.as_str().len())
                    .saturating_add(value.as_bytes().len())
            });
        let query = provider_wire_query(provider);
        let bytes = query.iter().fold(bytes, |total, (name, value)| {
            total.saturating_add(name.len()).saturating_add(value.len())
        });
        if bytes > MAX_ENTRY_BYTES || headers.len() > 128 || query.len() > 128 {
            tracing::warn!(
                "Credential snapshot exceeds identity budget; opaque replay is unvalidated"
            );
            return Ok(None);
        }
        if let Some(entry) = self.entries.iter().find(|entry| {
            entry.headers == headers && entry.base_url == provider.base_url && entry.query == query
        }) {
            return Ok(Some(entry.id.clone()));
        }
        let id = random_id()?;
        // Rebuild rather than retain oversized backing capacities/sliced values
        // from caller-owned maps. All cached header values remain sensitive.
        let mut compact_headers = HeaderMap::with_capacity(headers.len());
        for (name, value) in &headers {
            let name = http::HeaderName::from_bytes(name.as_str().as_bytes())
                .map_err(|_| AuthError::Build("invalid credential header name".into()))?;
            let mut value = http::HeaderValue::from_bytes(value.as_bytes())
                .map_err(|_| AuthError::Build("invalid credential header value".into()))?;
            value.set_sensitive(true);
            compact_headers.append(name, value);
        }
        if self.entries.len() == MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            headers: compact_headers,
            base_url: provider.base_url.clone(),
            query,
            id: id.clone(),
        });
        Ok(Some(id))
    }
}

/// The exact query pairs the wire transport sends for this provider:
/// URL-embedded pairs in URL order, then sorted configured pairs. No value
/// is ever hashed, serialized, or logged; this lives only in the private
/// bounded cache.
fn provider_wire_query(provider: &Provider) -> Vec<(String, String)> {
    let mut query: Vec<(String, String)> = url::Url::parse(&provider.base_url)
        .map(|url| {
            url.query_pairs()
                .map(|(name, value)| (name.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(params) = &provider.query_params {
        let mut configured: Vec<(String, String)> = params
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        configured.sort();
        query.extend(configured);
    }
    query
}

/// Compares immutable actual headers plus the complete private URI/query.
/// Only the random identity may be persisted. Eviction/restart conservatively
/// yields a new identity even if an old credential is presented again.
pub fn credential_instance_identity(
    provider: &Provider,
    headers: HeaderMap,
) -> Result<Option<String>, AuthError> {
    static CACHE: OnceLock<Mutex<InstanceCache>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| AuthError::Build("credential identity cache is unavailable".into()))?;
    cache.identify(provider, headers, || {
        let mut bytes = [0u8; 16];
        rand::rngs::OsRng.try_fill_bytes(&mut bytes).map_err(|_| {
            AuthError::Build("credential identity randomness is unavailable".into())
        })?;
        Ok(format!(
            "credential-instance-v1:{}",
            uuid::Uuid::from_bytes(bytes)
        ))
    })
}

#[cfg(test)]
#[path = "credential_instance_tests.rs"]
mod tests;
