//! Pins the rig 0.47 stream-failure taxonomy onto codex's transport taxonomy.

use super::map_provider_error;
use codex_api::ApiError;
use codex_api::TransportError;
use rig_core::error::ProviderError;
use std::time::Duration;

#[test]
fn stalled_and_truncated_streams_are_retryable_transport_failures() {
    for error in [
        ProviderError::Stalled(Duration::from_secs(300)),
        ProviderError::Truncated,
    ] {
        match map_provider_error(error) {
            ApiError::Transport(TransportError::Network(_)) => {}
            other => panic!("expected a retryable network failure, got {other:?}"),
        }
    }
}

#[test]
fn stream_ended_from_the_bridge_maps_to_a_retryable_network_failure() {
    let error = ProviderError::Http(std::sync::Arc::new(
        rig_core::http_client::Error::StreamEnded,
    ));
    assert!(matches!(
        map_provider_error(error),
        ApiError::Transport(TransportError::Network(_))
    ));
}

#[test]
fn an_unsupported_option_stays_a_non_retryable_invalid_request() {
    // The typed effort channel must keep failing fast: retrying a tier the
    // model does not have would spend paid attempts and hide the real cause.
    let error = ProviderError::UnsupportedOption(rig_core::completion::UnsupportedOption::new(
        "reasoning",
        "test-provider",
        "test-model",
        "model does not accept this tier",
    ));
    assert!(matches!(
        map_provider_error(error),
        ApiError::InvalidRequest { .. }
    ));
}
