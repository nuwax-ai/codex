//! Executes pooled HTTP requests, redirects, and TLS fallback within one request lifetime.

use std::future::Future;
use std::io;
use std::sync::atomic::Ordering;

use futures::FutureExt;
use futures::future::BoxFuture;
use http::header::PROXY_AUTHORIZATION;

use super::RouteAwareClientPool;
#[cfg(windows)]
use super::RouteAwareClientPoolError;
use super::RouteAwareRequestError;
use super::SelectedTlsBackend;
use crate::HttpResponse;
use crate::OutboundProxyRoute;
#[cfg(windows)]
use crate::RouteFailureClass;
#[cfg(windows)]
use crate::client::TransportClient;
use crate::client::apply_default_headers;
use crate::request_builder::DestinationGuard;
use crate::request_draft::RequestDraft;
use crate::route_aware_redirect::MAX_REDIRECTS;
use crate::route_aware_redirect::insert_referer;
use crate::route_aware_redirect::is_redirect;
use crate::route_aware_redirect::redirect_request;
use crate::route_aware_redirect::redirect_url;
use crate::route_aware_redirect::remove_sensitive_headers;
use crate::tls_backend_fallback::should_retry_with_rustls;
#[cfg(windows)]
use crate::windows_proxy_auth::ProxyPipe;

impl RouteAwareClientPool {
    /// Executes requests through shared routing, destination, and redirect checks.
    ///
    /// Box the execution future so nested callers do not inherit policy and routing state.
    pub fn execute(
        &self,
        request: reqwest::Request,
    ) -> BoxFuture<'_, Result<HttpResponse, RouteAwareRequestError>> {
        self.send(request.into())
    }

    pub(crate) fn send(
        &self,
        request: RequestDraft,
    ) -> BoxFuture<'_, Result<HttpResponse, RouteAwareRequestError>> {
        self.send_guarded(request, None)
    }

    /// [`Self::send`] with a policy that refuses destinations hop by hop.
    ///
    /// While a guard is attached, a client that follows redirects has them
    /// followed by the pool instead, because a transport-level redirect never
    /// reaches this policy. A client built without redirects is left alone: it
    /// never opens another hop for the guard to check.
    pub(crate) fn send_guarded(
        &self,
        request: RequestDraft,
        destination_guard: Option<DestinationGuard>,
    ) -> BoxFuture<'_, Result<HttpResponse, RouteAwareRequestError>> {
        self.send_with_resolver_guarded(
            request,
            |request_url| {
                self.http_client_factory
                    .resolve_proxy_route_async(request_url)
            },
            destination_guard,
        )
        .boxed()
    }

    /// Test-only spelling of [`Self::send_with_resolver_guarded`] without a guard.
    #[cfg(test)]
    pub(super) async fn send_with_resolver<F, Fut>(
        &self,
        request: impl Into<RequestDraft>,
        resolve_route: F,
    ) -> Result<HttpResponse, RouteAwareRequestError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = io::Result<OutboundProxyRoute>>,
    {
        self.send_with_resolver_guarded(request, resolve_route, None)
            .await
    }

    pub(super) async fn send_with_resolver_guarded<F, Fut>(
        &self,
        request: impl Into<RequestDraft>,
        resolve_route: F,
        destination_guard: Option<DestinationGuard>,
    ) -> Result<HttpResponse, RouteAwareRequestError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = io::Result<OutboundProxyRoute>>,
    {
        let request = request.into();
        let permit = self
            .http_client_factory
            .network_policy()
            .acquire(request.request.url())?;
        permit
            .run(Box::pin(self.send_authorized(
                request,
                resolve_route,
                destination_guard,
            )))
            .await?
    }

    async fn send_authorized<F, Fut>(
        &self,
        draft: RequestDraft,
        resolve_route: F,
        destination_guard: Option<DestinationGuard>,
    ) -> Result<HttpResponse, RouteAwareRequestError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = io::Result<OutboundProxyRoute>>,
    {
        let mut request = draft.request;
        let mut draft_headers = Some(draft.headers);
        let request_method = request.method().clone();
        let mut request_url = request.url().to_string();
        // A guard has to see every hop, so on a client that WOULD follow
        // redirects the pool follows them here instead: the transport's own
        // redirect handling would never consult the guard. A client built
        // without redirects never opens a second hop, so it keeps handing the
        // redirect itself back to the caller — the guard decides who checks a
        // hop, not whether one is taken.
        let follows_redirects_manually = self.follows_redirects_manually()
            || (destination_guard.is_some() && self.client_builder.follows_redirects());
        let timeout_deadline = request
            .timeout()
            .copied()
            .map(|timeout| tokio::time::Instant::now() + timeout);
        let mut redirects = 0;
        let mut previous_route = None;
        loop {
            let current_url = request.url().clone();
            // The same predicate the caller applied to the URL it built now
            // applies to the URL this hop would really connect to.
            if let Some(guard) = destination_guard.as_ref()
                && let Some(reason) = guard(current_url.as_str())
            {
                return Err(RouteAwareRequestError::Build(reason));
            }
            let permit = self
                .http_client_factory
                .network_policy()
                .acquire(&current_url)?;
            let hop = async {
                let (current_route, mut client, selected_tls_backend) = self
                    .client_for_url_with_resolver(
                        current_url.as_str(),
                        &resolve_route,
                        follows_redirects_manually,
                    )
                    .await?;
                client.request_logging = self.client_builder.request_logging;
                if let Some(headers) = draft_headers.take() {
                    request = RequestDraft { request, headers }.build(&client)?;
                    request_url = request.url().to_string();
                    apply_default_headers(request.headers_mut(), &self.default_headers);
                }
                let current_url = request.url().clone();
                if previous_route
                    .as_ref()
                    .is_some_and(|previous_route| previous_route != &current_route)
                {
                    request.headers_mut().remove(PROXY_AUTHORIZATION);
                }
                previous_route = Some(current_route.clone());
                set_remaining_timeout(&mut request, timeout_deadline)?;
                let method = request.method().clone();
                let headers = request.headers().clone();
                let version = request.version();
                let timeout = request.timeout().copied();
                let replay = request.try_clone();
                let result = if follows_redirects_manually {
                    client.execute_without_request_logging(request).await
                } else {
                    client.execute(request).await
                }
                .map_err(RouteAwareRequestError::from);
                let result = match result {
                    // A failed CONNECT identifies a proxy challenge. An origin 407 is a response.
                    #[cfg(windows)]
                    Err(error)
                        if error.is_connect()
                            && error.failure_class()
                                == Some(RouteFailureClass::ProxyAuthenticationRequired) =>
                    {
                        self.retry_windows_proxy(
                            &current_route,
                            selected_tls_backend,
                            replay.as_ref(),
                            error,
                            timeout_deadline,
                            follows_redirects_manually,
                        )
                        .await
                    }
                    Err(RouteAwareRequestError::Request(error)) => {
                        self.retry_with_rustls(
                            &current_route,
                            selected_tls_backend,
                            replay.as_ref(),
                            #[cfg(windows)]
                            /*proxy_pipe*/
                            None,
                            error,
                            timeout_deadline,
                            follows_redirects_manually,
                        )
                        .await
                    }
                    result => result,
                };
                if follows_redirects_manually
                    && let Err(RouteAwareRequestError::Request(error)) = &result
                {
                    client.log_error_summary(&request_method, &request_url, error);
                }
                let response = result?;
                let next_request = if follows_redirects_manually && is_redirect(response.status()) {
                    redirect_url(&response).and_then(|next_url| {
                        redirect_request(
                            response.status(),
                            method,
                            headers,
                            version,
                            timeout,
                            replay,
                            next_url,
                        )
                    })
                } else {
                    None
                };
                if follows_redirects_manually && next_request.is_none() {
                    client.log_response(&request_method, &request_url, &response);
                }
                Ok::<_, RouteAwareRequestError>((response, next_request, current_url))
            };
            let (response, next_request, current_url) = permit
                .run(async {
                    match timeout_deadline {
                        Some(deadline) => tokio::time::timeout_at(deadline, hop)
                            .await
                            .map_err(|_| RouteAwareRequestError::Timeout)?,
                        None => hop.await,
                    }
                })
                .await??;
            let Some(mut next_request) = next_request else {
                return Ok(HttpResponse::new(response, permit));
            };
            if let Some(redirect_observed) = &self.client_builder.redirect_observed {
                redirect_observed.store(/*val*/ true, Ordering::Relaxed);
            }
            let next_request_url = next_request.url().clone();
            if !matches!(next_request_url.scheme(), "http" | "https") {
                return Err(RouteAwareRequestError::UnsupportedRedirectScheme(
                    next_request_url.scheme().to_string(),
                ));
            }
            if redirects >= MAX_REDIRECTS {
                return Err(RouteAwareRequestError::TooManyRedirects);
            }
            remove_sensitive_headers(
                next_request.headers_mut(),
                &current_url,
                &next_request_url,
                self.http_client_factory.protects_sensitive_headers(),
            );
            insert_referer(next_request.headers_mut(), &current_url, &next_request_url);
            request = next_request;
            redirects += 1;
        }
    }

    pub(super) async fn retry_with_rustls(
        &self,
        current_route: &OutboundProxyRoute,
        selected_tls_backend: SelectedTlsBackend,
        replay: Option<&reqwest::Request>,
        #[cfg(windows)] proxy_pipe: Option<&ProxyPipe>,
        error: reqwest::Error,
        timeout_deadline: Option<tokio::time::Instant>,
        follows_redirects_manually: bool,
    ) -> Result<reqwest::Response, RouteAwareRequestError> {
        let Some(rustls_clients) = self.rustls_clients.as_ref() else {
            return Err(error.into());
        };
        let Some(replay) = replay else {
            return Err(error.into());
        };
        let current_url = replay.url();

        if current_url.scheme() != "https"
            || selected_tls_backend == SelectedTlsBackend::RustlsFallback
            || !should_retry_with_rustls(&error)
        {
            return Err(error.into());
        }

        let Some(mut retry_request) = replay.try_clone() else {
            return Err(error.into());
        };

        let candidate = rustls_clients.client_for_route(current_route);
        #[cfg(windows)]
        let candidate = match proxy_pipe {
            Some(pipe) => Some(self.windows_proxy_client(
                current_route,
                SelectedTlsBackend::RustlsFallback,
                pipe,
            )?),
            None => candidate,
        };
        let mut fallback_client = match candidate {
            Some(client) => client,
            None => self.rustls_client_for_route(current_route, follows_redirects_manually)?,
        };
        fallback_client.request_logging = self.client_builder.request_logging;

        set_remaining_timeout(&mut retry_request, timeout_deadline)?;

        let response = if follows_redirects_manually {
            fallback_client
                .execute_without_request_logging(retry_request)
                .await
        } else {
            fallback_client.execute(retry_request).await
        }?;

        tracing::info!(
            event.name = "codex.http_client.tls_backend_fallback",
            "HTTP client switched to rustls after a TLS protocol negotiation failure"
        );

        // A Windows auth pipe serves this destination alone, so it cannot enter the route cache.
        #[cfg(windows)]
        if proxy_pipe.is_some() {
            return Ok(response);
        }
        rustls_clients.remember(current_url, current_route, fallback_client);
        Ok(response)
    }

    #[cfg(windows)]
    async fn retry_windows_proxy(
        &self,
        current_route: &OutboundProxyRoute,
        selected_tls_backend: SelectedTlsBackend,
        replay: Option<&reqwest::Request>,
        error: RouteAwareRequestError,
        timeout_deadline: Option<tokio::time::Instant>,
        follows_redirects_manually: bool,
    ) -> Result<reqwest::Response, RouteAwareRequestError> {
        let OutboundProxyRoute::Proxy { url, .. } = current_route else {
            return Err(error);
        };
        let Some(mut request) = replay.and_then(reqwest::Request::try_clone) else {
            return Err(error);
        };
        if request.url().scheme() != "https" {
            return Err(error);
        }

        let pipe = match crate::windows_proxy_auth::pipe_for(
            url,
            request.url(),
            &self.client_builder.tls,
            self.client_builder.connect_timeout,
        )
        .await
        {
            Ok(pipe) => pipe,
            Err(auth_error) if auth_error.kind() == io::ErrorKind::TimedOut => {
                return Err(RouteAwareRequestError::Timeout);
            }
            Err(_) => return Err(error),
        };
        let client = self.windows_proxy_client(current_route, selected_tls_backend, &pipe)?;
        set_remaining_timeout(&mut request, timeout_deadline)?;
        // Keep the listener alive through a possible TLS retry.
        let result = client.execute_without_request_logging(request).await;
        match result {
            Ok(response) => Ok(response),
            Err(error) => {
                self.retry_with_rustls(
                    current_route,
                    selected_tls_backend,
                    replay,
                    Some(&pipe),
                    error,
                    timeout_deadline,
                    follows_redirects_manually,
                )
                .await
            }
        }
    }

    #[cfg(windows)]
    fn windows_proxy_client(
        &self,
        route: &OutboundProxyRoute,
        backend: SelectedTlsBackend,
        pipe: &ProxyPipe,
    ) -> Result<TransportClient, RouteAwareClientPoolError> {
        let mut builder = self
            .client_builder
            .clone()
            .without_redirects()
            .with_windows_proxy_pipe(pipe);
        if backend == SelectedTlsBackend::RustlsFallback {
            builder = builder.with_rustls_tls();
        }
        builder
            .build_for_resolved_route(&self.http_client_factory, self.route_class, route)
            .map_err(Into::into)
    }
}

fn set_remaining_timeout(
    request: &mut reqwest::Request,
    deadline: Option<tokio::time::Instant>,
) -> Result<(), RouteAwareRequestError> {
    if let Some(deadline) = deadline {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(RouteAwareRequestError::Timeout)?;
        *request.timeout_mut() = Some(remaining);
    }
    Ok(())
}
