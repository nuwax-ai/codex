//! Error-path probing: drives a turn start against a broken endpoint and
//! classifies the failure (auth, status, timeout) instead of a hang.

use super::*;

// ================================================================
// Error-path probing
// ================================================================

/// Starts a turn and returns the bridge's start error (if any) without
/// draining — used by the auth-rejected scenario to assert HTTP status
/// passthrough (401 must reach codex-core's re-login loop as Http{401}).
pub async fn turn_start_error(
    cfg: &LiveConfig,
    base_url: &str,
    bridge: Bridge,
    request: &ResponsesApiRequest,
) -> Option<String> {
    let bad_auth: SharedAuthProvider = Arc::new(StaticBearerAuth(format!(
        "invalid-key-{}",
        cfg.api_key.len()
    )));
    let provider = vendor_provider(&cfg.vendor, base_url);
    let result = match bridge {
        Bridge::Genai => {
            let adapter_kind = if codex_rust_rig_bridge::protocol_for_base_url(base_url)
                == codex_rust_rig_bridge::RigProtocol::Anthropic
            {
                genai::adapter::AdapterKind::Anthropic
            } else {
                genai::adapter::AdapterKind::OpenAI
            };
            timeout(
                TURN_TIMEOUT,
                codex_rust_genai_bridge::stream_via_genai(
                    request,
                    &provider,
                    &bad_auth,
                    HeaderMap::new(),
                    adapter_kind,
                    provider.stream_idle_timeout,
                ),
            )
            .await
        }
        Bridge::Rig => {
            timeout(
                TURN_TIMEOUT,
                codex_rust_rig_bridge::stream_via_rig(
                    request,
                    &provider,
                    &bad_auth,
                    HeaderMap::new(),
                    codex_rust_rig_bridge::RigProtocol::from_base_url(base_url),
                    provider.stream_idle_timeout,
                ),
            )
            .await
        }
    };
    match result {
        Ok(Ok(mut stream)) => {
            // genai (and any bridge that defers HTTP failures into the
            // stream) surfaces a bad key as the first error EVENT — drain
            // until it arrives so the scenario can assert on it either way.
            loop {
                match timeout(TURN_TIMEOUT, stream.rx_event.recv()).await {
                    Ok(Some(Err(api_error))) => return Some(format!("{api_error:#}")),
                    // A completed turn means the request unexpectedly
                    // succeeded despite the invalid key.
                    Ok(Some(Ok(ResponseEvent::Completed { .. }))) => return None,
                    Ok(None) => return None,
                    Err(_elapsed) => return Some("drain timed out".to_string()),
                    Ok(Some(Ok(_))) => {}
                }
            }
        }
        Ok(Err(api_error)) => Some(format!("{api_error:#}")),
        Err(_elapsed) => Some("start timed out".to_string()),
    }
}
