//! Builds direct and routed requests without constructing a second transport client.
//! Routed requests select their transport only when sent; debug output redacts their URLs.

use bytes::Bytes;
use futures::TryStream;
use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use http::Method;
use http::header::AUTHORIZATION;
use http::header::CONTENT_TYPE;
use reqwest::IntoUrl;
use serde::Serialize;
use std::fmt;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use crate::HttpError;
use crate::HttpResponse;
use crate::RouteAwareClientPool;
use crate::client::HttpClientBackend;
use crate::client::TransportClient;
use crate::request_draft::HeaderUpdate;
use crate::request_draft::RequestDraft;

#[must_use = "requests are not sent unless `send` is awaited"]
pub struct RequestBuilder {
    /// `None` only for a request that is refused before it is built; see
    /// [`RequestBuilder::refused`].
    backend: Option<HttpClientBackend>,
    request: Result<RequestDraft, HttpError>,
    /// Consulted for every destination the request connects to, redirects
    /// included. `Some(reason)` refuses that hop before a route is resolved or
    /// a name is looked up.
    destination_guard: Option<DestinationGuard>,
}

/// A per-destination policy the transport applies to every hop of one request.
///
/// A caller that refuses a destination only by looking at the URL it was handed
/// leaves redirects unguarded: the transport follows a `Location` itself and
/// would connect to a host the caller never saw. Attach the same predicate here
/// and the refusal covers each hop.
pub type DestinationGuard = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

impl fmt::Debug for RequestBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestBuilder")
            .field(
                "method",
                &self
                    .request
                    .as_ref()
                    .ok()
                    .map(|request| request.request.method()),
            )
            .field("url", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl RequestBuilder {
    pub(crate) fn direct<U: IntoUrl>(client: &TransportClient, method: Method, url: U) -> Self {
        Self {
            backend: Some(HttpClientBackend::Direct(client.clone())),
            request: RequestDraft::new(method, url).map_err(HttpError::Request),
            destination_guard: None,
        }
    }

    pub(crate) fn routed<U: IntoUrl>(
        pool: Arc<RouteAwareClientPool>,
        method: Method,
        url: U,
    ) -> Self {
        Self {
            backend: Some(HttpClientBackend::Routed(pool)),
            request: RequestDraft::new(method, url).map_err(HttpError::Request),
            destination_guard: None,
        }
    }

    /// A request that is never sent: awaiting [`Self::send`] fails with `reason`
    /// before a route is selected or a name resolved, so nothing leaves the
    /// process. For a caller that is refused by policy rather than by the
    /// network, so that retrying cannot help and the operator reads why.
    pub fn refused(reason: impl Into<String>) -> Self {
        Self {
            backend: None,
            request: Err(HttpError::Build(reason.into())),
            destination_guard: None,
        }
    }

    /// Applies `guard` to every hop this request takes, redirects included.
    ///
    /// On a client that follows redirects the pool follows them itself while a
    /// guard is attached, so the refusal cannot be skipped by a transport-level
    /// redirect. Whether redirects are followed at all stays the client's
    /// decision: one built without redirects still returns them unanswered.
    pub fn destination_guard(mut self, guard: DestinationGuard) -> Self {
        self.destination_guard = Some(guard);
        self
    }

    fn map(mut self, update: impl FnOnce(&mut RequestDraft) -> Result<(), HttpError>) -> Self {
        self.request = self.request.and_then(|mut request| {
            update(&mut request)?;
            Ok(request)
        });
        self
    }

    pub fn headers(self, headers: HeaderMap) -> Self {
        self.map(|request| {
            request.extend_headers(headers);
            Ok(())
        })
    }

    pub fn header<K, V>(self, key: K, value: V) -> Self
    where
        HeaderName: TryFrom<K>,
        <HeaderName as TryFrom<K>>::Error: Into<http::Error>,
        HeaderValue: TryFrom<V>,
        <HeaderValue as TryFrom<V>>::Error: Into<http::Error>,
    {
        self.map(|request| {
            let key = HeaderName::try_from(key)
                .map_err(Into::into)
                .map_err(|error: http::Error| HttpError::Build(error.to_string()))?;
            let value = HeaderValue::try_from(value)
                .map_err(Into::into)
                .map_err(|error: http::Error| HttpError::Build(error.to_string()))?;
            request.headers.push(HeaderUpdate::Append(key, value));
            Ok(())
        })
    }

    pub fn bearer_auth<T: Display>(self, token: T) -> Self {
        self.map(|request| {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|error| HttpError::Build(error.to_string()))?;
            value.set_sensitive(true);
            request
                .headers
                .push(HeaderUpdate::Append(AUTHORIZATION, value));
            Ok(())
        })
    }

    /// Bounds route selection, connection establishment, and the response headers.
    /// Use [`crate::HttpClientBuilder::connect_timeout`] to bound only connection establishment.
    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|request| {
            *request.request.timeout_mut() = Some(timeout);
            Ok(())
        })
    }

    pub fn version(self, version: http::Version) -> Self {
        self.map(|request| {
            *request.request.version_mut() = version;
            Ok(())
        })
    }

    pub fn json<T: ?Sized + Serialize>(self, value: &T) -> Self {
        self.map(|request| {
            let body =
                serde_json::to_vec(value).map_err(|error| HttpError::Build(error.to_string()))?;
            let has_content_type = request.headers.iter().any(|update| match update {
                HeaderUpdate::Replace(headers) => headers.contains_key(CONTENT_TYPE),
                HeaderUpdate::Append(name, _) => *name == CONTENT_TYPE,
            });
            if !has_content_type {
                request.headers.push(HeaderUpdate::Append(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                ));
            }
            *request.request.body_mut() = Some(body.into());
            Ok(())
        })
    }

    pub fn query<T: ?Sized + Serialize>(self, query: &T) -> Self {
        self.map(|request| {
            query
                .serialize(serde_urlencoded::Serializer::new(
                    &mut request.request.url_mut().query_pairs_mut(),
                ))
                .map_err(|error| HttpError::Build(error.to_string()))?;
            if request.request.url().query() == Some("") {
                request.request.url_mut().set_query(None);
            }
            Ok(())
        })
    }

    pub fn body<B: Into<reqwest::Body>>(self, body: B) -> Self {
        self.map(|request| {
            *request.request.body_mut() = Some(body.into());
            Ok(())
        })
    }

    /// Sets a streaming body without exposing the underlying HTTP implementation.
    pub fn body_stream<S>(self, stream: S) -> Self
    where
        S: TryStream + Send + 'static,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        Bytes: From<S::Ok>,
    {
        self.body(reqwest::Body::wrap_stream(stream))
    }

    pub async fn send(self) -> Result<HttpResponse, HttpError> {
        // A request that failed to build is reported before its transport is
        // touched, which is also what makes a refused request never leave.
        let request = self.request?;
        match self.backend {
            Some(HttpClientBackend::Routed(pool)) => {
                pool.send_guarded(request, self.destination_guard).await
            }
            Some(HttpClientBackend::Direct(client)) => {
                // A direct client's transport follows redirects itself and
                // never calls back here, so a guard could not cover its hops.
                // Refusing beats enforcing nothing while the caller believes
                // the destination is policed.
                if self.destination_guard.is_some() {
                    return Err(HttpError::Build(
                        "a destination guard needs a routed transport; this request uses a fixed client"
                            .into(),
                    ));
                }
                Ok(client.execute(request.build(&client)?).await?.into())
            }
            None => Err(HttpError::Build(
                "request has no transport to send it with".into(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "request_builder_tests.rs"]
mod tests;
