//! Retry only the HTTP handshake; a successful SSE response leaves this loop.

use bytes::Bytes;
use codex_api::RetryConfig;
use codex_api::TransportError;
use codex_http_client::RetryAfter;
use rig_core::http_client::Error;
use rig_core::http_client::HttpClientExt;
use rig_core::http_client::Request;
use rig_core::http_client::StreamingResponse;

pub(crate) async fn send_streaming(
    client: &reqwest_rig::Client,
    request: Request<Bytes>,
    retry: Option<&RetryConfig>,
    recorder: &crate::FinalRequestRecorder,
) -> Result<StreamingResponse, Error> {
    let policy = retry.map(RetryConfig::to_policy);
    let max_retries = policy
        .as_ref()
        .map_or(/*default*/ 0, |policy| policy.max_attempts);
    let (parts, body) = request.into_parts();
    for attempt in 0..=max_retries {
        let mut request = Request::new(body.clone());
        *request.method_mut() = parts.method.clone();
        *request.uri_mut() = parts.uri.clone();
        *request.version_mut() = parts.version;
        *request.headers_mut() = parts.headers.clone();
        *request.extensions_mut() = parts.extensions.clone();
        crate::request_capture::record(recorder, &request)?;
        match client.send_streaming(request).await {
            Ok(response) => return Ok(response),
            Err(error) => {
                let Some(policy) = &policy else {
                    return Err(error);
                };
                let Some(classification) = classify(&error) else {
                    return Err(error);
                };
                if !policy
                    .retry_on
                    .should_retry(&classification, attempt, max_retries)
                {
                    return Err(error);
                }
                let advice = error
                    .non_success_headers()
                    .and_then(RetryAfter::from_headers);
                let delay = advice
                    .map(RetryAfter::remaining_delay)
                    .unwrap_or_else(|| codex_client::backoff(policy.base_delay, attempt + 1));
                codex_client::record_retry!(
                    attempt + 1,
                    delay,
                    codex_client::RetryOperation::HttpRequest
                );
                if let Some(advice) = advice {
                    tokio::time::sleep_until(advice.deadline()).await;
                } else {
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    // All non-retryable/final errors return from the loop, including zero retries.
    Err(Error::Instance(Box::new(TransportError::RetryLimit)))
}

fn classify(error: &Error) -> Option<TransportError> {
    match error {
        Error::InvalidStatusCode(status)
        | Error::InvalidStatusCodeWithMessage(status, _)
        | Error::InvalidStatusCodeWithDetails { status, .. } => Some(TransportError::Http {
            status: *status,
            url: None,
            headers: None,
            body: None,
            retry_after: None,
        }),
        Error::Instance(error) => {
            let error = error.downcast_ref::<reqwest_rig::Error>()?;
            if error.is_timeout() {
                Some(TransportError::Timeout)
            } else if error.is_connect() || error.is_request() || error.is_body() {
                Some(TransportError::Network("Rig HTTP request failed".into()))
            } else {
                None
            }
        }
        Error::Protocol(_)
        | Error::InvalidHeaderValue(_)
        | Error::NoHeaders
        | Error::StreamEnded
        | Error::InvalidContentType(_) => None,
    }
}

#[cfg(test)]
#[path = "request_retry_tests.rs"]
mod tests;
