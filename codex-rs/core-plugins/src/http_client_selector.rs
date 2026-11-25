use codex_http_client::DestinationGuard;
use codex_http_client::OutboundProxyPolicy;
use codex_http_client::RequestBuilder;
use codex_http_client::RouteAwareClientPool;
use codex_http_client::openai_egress_enabled;
use http::Method;
use std::fmt;
use std::fmt::Debug;
use std::sync::Arc;
use url::Url;

/// Builds requests whose URL is also used to resolve their outbound route.
///
/// Implementations must keep route selection coupled to the request URL. Returning a transport
/// client would let callers send a different URL than the one used for route selection.
pub(crate) trait HttpClientSelector: Debug + Send + Sync {
    fn request(&self, method: Method, url: &str) -> RequestBuilder;
    fn outbound_proxy_policy(&self) -> OutboundProxyPolicy;
}

impl HttpClientSelector for RouteAwareClientPool {
    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        RouteAwareClientPool::request(self, method, url)
    }

    fn outbound_proxy_policy(&self) -> OutboundProxyPolicy {
        RouteAwareClientPool::outbound_proxy_policy(self)
    }
}

/// Fork (nuwax-codex): what a request refused by the gate says.
pub(crate) const OPENAI_EGRESS_CLOSED: &str = "OpenAI first-party egress is closed by default; set NUWAX_OPENAI_EGRESS=1 to allow requests to OpenAI-operated hosts";

/// The plugin service's client pool behind the OpenAI first-party egress gate.
///
/// The plugin directory, featured-plugin ids, bundle, share and search
/// endpoints, and the curated plugin sync all run on infrastructure OpenAI
/// operates. A deployment that reaches vendor models through its own gateway
/// must not show those names in a packet capture, so the gate sits where the
/// requests are built: while it is closed, every request to an OpenAI-operated
/// host is refused inside the process with a reason, whatever entrypoint
/// started it. `exec` and the TUI reach the plugin service through core,
/// outside the app-server's startup switch, which is why the gate is here and
/// not only there.
///
/// It is decided per destination, not per process: a host the operator runs
/// themselves — a plugin directory mirrored inside their network — is not
/// OpenAI's, and keeps working with the gate closed.
pub(crate) struct OpenAiEgressGatedClientPool {
    inner: RouteAwareClientPool,
    egress_enabled: bool,
}

impl OpenAiEgressGatedClientPool {
    pub(crate) fn new(inner: RouteAwareClientPool) -> Self {
        Self::with_egress(inner, openai_egress_enabled())
    }

    /// [`Self::new`] with the gate's state given, for tests: they must not
    /// depend on the process environment.
    pub(crate) fn with_egress(inner: RouteAwareClientPool, egress_enabled: bool) -> Self {
        Self {
            inner,
            egress_enabled,
        }
    }
}

impl fmt::Debug for OpenAiEgressGatedClientPool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiEgressGatedClientPool")
            .field("egress_enabled", &self.egress_enabled)
            .finish_non_exhaustive()
    }
}

impl OpenAiEgressGatedClientPool {
    /// The initial-URL predicate, ready to ride every hop of one request.
    ///
    /// A mirror the operator configured can answer with a redirect to an
    /// OpenAI-operated host; only a per-hop check keeps the gate's promise that
    /// those names never reach a packet capture.
    fn destination_guard(&self) -> DestinationGuard {
        Arc::new(|url: &str| is_openai_operated_url(url).then(|| OPENAI_EGRESS_CLOSED.to_string()))
    }
}

impl HttpClientSelector for OpenAiEgressGatedClientPool {
    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        if self.egress_enabled {
            return self.inner.request(method, url);
        }
        if !is_openai_operated_url(url) {
            // Allowed here, but the hop that matters may be a redirect target.
            return self
                .inner
                .request(method, url)
                .destination_guard(self.destination_guard());
        }
        tracing::debug!(
            host = Url::parse(url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned)),
            "refusing a plugin request: OpenAI first-party egress is closed"
        );
        RequestBuilder::refused(OPENAI_EGRESS_CLOSED)
    }

    fn outbound_proxy_policy(&self) -> OutboundProxyPolicy {
        self.inner.outbound_proxy_policy()
    }
}

/// Whether `url` addresses infrastructure OpenAI operates: ChatGPT's backend,
/// OpenAI's API, or the `openai` organisation's repositories on GitHub. The
/// organisation's release-asset downloads are not recognised; they are
/// reached only through a URL the directory hands out, which the gate already
/// refuses.
pub(crate) fn is_openai_operated_url(url: &str) -> bool {
    let Ok(url) = Url::parse(url) else {
        // Nothing a transport can send as it is: leave the verdict to it.
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if matches!(host.as_str(), "chatgpt.com" | "openai.com")
        || host.ends_with(".chatgpt.com")
        || host.ends_with(".openai.com")
    {
        return true;
    }
    let github_host = matches!(
        host.as_str(),
        "github.com"
            | "www.github.com"
            | "api.github.com"
            | "codeload.github.com"
            | "raw.githubusercontent.com"
    );
    if !github_host {
        return false;
    }
    let mut segments = url.path().trim_matches('/').split('/');
    if host == "api.github.com" {
        // Repository routes are `/repos/{owner}/{repo}`.
        return matches!(
            (segments.next(), segments.next()),
            (Some("repos"), Some("openai"))
        );
    }
    segments.next() == Some("openai")
}

#[cfg(test)]
#[path = "http_client_selector_tests.rs"]
mod tests;
