//! Final primary-auth headers derived from the actual resolved credential set.
//! Presence is meaningful: an explicit empty value is never treated as absence.

use crate::client::RigProtocol;
use codex_api::ApiError;
use http::HeaderMap;
use http::HeaderValue;

#[derive(Clone)]
pub(crate) struct WireAuth {
    pub(crate) headers: HeaderMap,
}

/// Returns Rig's SDK key plus the authoritative primary headers after the
/// established protocol mapping. Gateway/header credentials remain intact.
pub(crate) fn resolved_sdk_auth(
    headers: &HeaderMap,
    protocol: RigProtocol,
) -> Result<(String, WireAuth), ApiError> {
    let bearer = crate::client::bearer_token(headers);
    let explicit_key = if protocol == RigProtocol::Anthropic {
        headers.get("x-api-key")
    } else {
        None
    };
    let key = if let Some(value) = explicit_key {
        Some(
            value
                .to_str()
                .map_err(|_| invalid_key("x-api-key"))?
                .to_string(),
        )
    } else if let Some(bearer) = bearer {
        Some(bearer.to_string())
    } else if let Some(value) = headers.get("api-key") {
        Some(
            value
                .to_str()
                .map_err(|_| invalid_key("api-key"))?
                .to_string(),
        )
    } else {
        None
    };
    let mut primary = HeaderMap::new();
    match protocol {
        RigProtocol::Responses => {
            if let Some(value) = headers.get(http::header::AUTHORIZATION) {
                primary.insert(http::header::AUTHORIZATION, value.clone());
            }
        }
        RigProtocol::Chat => {
            if let Some(value) = headers.get(http::header::AUTHORIZATION)
                && bearer.is_none()
            {
                primary.insert(http::header::AUTHORIZATION, value.clone());
            } else if let Some(key) = &key {
                // Retain the existing Chat api-key-only -> Bearer fallback
                // and SDK capitalization of a resolved bearer scheme.
                primary.insert(
                    http::header::AUTHORIZATION,
                    header_value(&format!("Bearer {key}"), "authorization")?,
                );
            }
        }
        RigProtocol::Anthropic => {
            if let Some(value) = headers.get(http::header::AUTHORIZATION)
                && (explicit_key.is_some() || bearer.is_none())
            {
                primary.insert(http::header::AUTHORIZATION, value.clone());
            }
            if let Some(key) = &key {
                primary.insert("x-api-key", header_value(key, "x-api-key")?);
            }
        }
    }
    for value in primary.values_mut() {
        value.set_sensitive(true);
    }
    Ok((key.unwrap_or_default(), WireAuth { headers: primary }))
}

fn header_value(value: &str, name: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(value).map_err(|_| invalid_key(name))
}

fn invalid_key(name: &str) -> ApiError {
    ApiError::InvalidRequest {
        message: format!("Invalid {name} header: the Rig SDK requires an ASCII credential value"),
    }
}
