//! The OpenAI first-party egress gate on the plugin service's HTTP clients.
//!
//! No test here touches the network: a closed gate refuses inside the process,
//! and an open gate is recognised by the transport's own destination-policy
//! denial, which fires before a name is resolved.

use super::HttpClientSelector;
use super::OPENAI_EGRESS_CLOSED;
use super::OpenAiEgressGatedClientPool;
use super::is_openai_operated_url;
use codex_http_client::ClientRouteClass;
use codex_http_client::DestinationPolicy;
use codex_http_client::NetworkPolicyController;
use codex_http_client::NetworkPolicyDenied;
use codex_http_client::RouteAwareClientPool;
use codex_http_client::RouteAwareRequestError;
use http::Method;
use std::collections::BTreeSet;
use std::time::Duration;

const OPENAI_URLS: &[&str] = &[
    "https://chatgpt.com/backend-api/plugins/featured?platform=codex",
    "https://chatgpt.com/backend-api/plugins/marketplaces",
    "https://api.openai.com/v1/models",
    "https://auth.openai.com/oauth/token",
    "https://github.com/openai/plugins.git/info/refs?service=git-upload-pack",
    "https://api.github.com/repos/openai/plugins/commits/main",
    "https://codeload.github.com/openai/plugins/zip/refs/heads/main",
    "https://raw.githubusercontent.com/openai/plugins/main/.codex-plugin/marketplace.json",
    "https://CHATGPT.com./backend-api/plugins",
];

/// The operator's own hosts: not OpenAI's, however they are spelled.
const OPERATOR_URLS: &[&str] = &[
    "https://plugins.corp.example/backend-api/plugins/featured",
    "http://127.0.0.1:8080/chatgpt.com/plugins/featured",
    "https://github.com/corp-example/plugins.git",
    "https://api.github.com/repos/corp-example/plugins",
    "https://raw.githubusercontent.com/corp-example/plugins/main/README.md",
    "https://openai-plugins.corp.example/v1/plugins",
    "https://notchatgpt.com/backend-api/plugins",
];

#[test]
fn openai_operated_destinations_are_recognised() {
    for url in OPENAI_URLS {
        assert!(is_openai_operated_url(url), "{url}");
    }
    for url in OPERATOR_URLS {
        assert!(!is_openai_operated_url(url), "{url}");
    }
    assert!(
        !is_openai_operated_url("not a url"),
        "what a transport cannot send is the transport's to report"
    );
}

/// A factory whose transport denies every destination before resolving a name.
fn denying_factory() -> codex_http_client::HttpClientFactory {
    let controller = NetworkPolicyController::default();
    let policy = controller.policy();
    controller.publish(
        policy.revision(),
        DestinationPolicy::Restricted {
            allowed_hosts: BTreeSet::new(),
        },
    );
    crate::test_support::test_http_client_factory().with_network_policy(policy)
}

fn api_pool(factory: &codex_http_client::HttpClientFactory) -> RouteAwareClientPool {
    RouteAwareClientPool::with_chatgpt_cloudflare_cookies_without_request_logging(
        factory.clone(),
        ClientRouteClass::Api,
    )
}

async fn send_error(pool: &OpenAiEgressGatedClientPool, url: &str) -> RouteAwareRequestError {
    tokio::time::timeout(
        Duration::from_secs(5),
        pool.request(Method::GET, url).send(),
    )
    .await
    .expect("no test waits on the network")
    .expect_err("every request in this module is denied by someone")
}

#[tokio::test]
async fn a_closed_gate_refuses_openai_hosts_inside_the_process() {
    let pool = OpenAiEgressGatedClientPool::with_egress(api_pool(&denying_factory()), false);
    for url in OPENAI_URLS {
        let error = send_error(&pool, url).await;
        let RouteAwareRequestError::Build(reason) = &error else {
            panic!("{url} must be refused by the gate, not reach the transport: {error:?}");
        };
        assert_eq!(reason, OPENAI_EGRESS_CLOSED, "{url}");
    }
}

#[tokio::test]
async fn an_open_gate_hands_openai_hosts_to_the_transport() {
    let pool = OpenAiEgressGatedClientPool::with_egress(api_pool(&denying_factory()), true);
    for url in OPENAI_URLS {
        let error = send_error(&pool, url).await;
        assert!(
            matches!(
                error,
                RouteAwareRequestError::Policy(NetworkPolicyDenied::Destination)
            ),
            "{url} must reach the transport once the operator opted in: {error:?}"
        );
    }
}

#[tokio::test]
async fn a_closed_gate_leaves_the_operators_own_hosts_alone() {
    let pool = OpenAiEgressGatedClientPool::with_egress(api_pool(&denying_factory()), false);
    for url in OPERATOR_URLS {
        let error = send_error(&pool, url).await;
        assert!(
            matches!(
                error,
                RouteAwareRequestError::Policy(NetworkPolicyDenied::Destination)
            ),
            "{url} is not OpenAI's: the gate must pass it to the transport: {error:?}"
        );
    }
}

/// The proof that matters for a host the operator runs: with the gate closed,
/// a request to it still leaves the process and is answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_gate_still_reaches_a_loopback_mirror() -> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = [0u8; 2048];
        let read = socket.read(&mut request).await?;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}",
            )
            .await?;
        Ok::<usize, std::io::Error>(read)
    });
    let factory = crate::test_support::test_http_client_factory();
    let pool = OpenAiEgressGatedClientPool::with_egress(api_pool(&factory), false);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        pool.request(
            Method::GET,
            &format!("http://{address}/backend-api/plugins/featured"),
        )
        .send(),
    )
    .await?;
    let response = response?;
    assert_eq!(response.status(), http::StatusCode::OK);
    let read = server.await??;
    assert!(read > 0);
    Ok(())
}

/// A mirror the operator configured is not OpenAI's, but the redirect it hands
/// back can be. With the gate closed the refusal must follow the request onto
/// that hop instead of stopping at the URL the caller built.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_closed_gate_refuses_the_openai_host_a_mirror_redirects_to()
-> Result<(), Box<dyn std::error::Error>> {
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::oneshot;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (served, mut served_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let mut request = [0u8; 2048];
        let read = socket.read(&mut request).await?;
        assert!(read > 0, "the mirror should receive the first hop");
        // The mirror answers with OpenAI's own backend, exactly as a misdirected
        // or compromised plugin directory could.
        socket
            .write_all(
                b"HTTP/1.1 302 Found\r\nlocation: https://chatgpt.com/backend-api/plugins/featured\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await?;
        let _ = served.send(());
        // Nothing else may arrive: the redirect target has to be refused before
        // a connection is attempted.
        let mut extra = [0u8; 64];
        Ok::<usize, std::io::Error>(socket.read(&mut extra).await.unwrap_or(0))
    });
    let factory = crate::test_support::test_http_client_factory();
    let pool = OpenAiEgressGatedClientPool::with_egress(api_pool(&factory), false);
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        pool.request(
            Method::GET,
            &format!("http://{address}/backend-api/plugins/featured"),
        )
        .send(),
    )
    .await?
    .expect_err("the redirect target is OpenAI's, so the gate must refuse it");
    let RouteAwareRequestError::Build(reason) = &error else {
        panic!("the gate must refuse the hop inside the process, not at the transport: {error:?}");
    };
    assert_eq!(reason, OPENAI_EGRESS_CLOSED);
    served_rx
        .try_recv()
        .expect("the mirror answered the first hop");
    assert_eq!(
        server.await??,
        0,
        "the refused hop must never reach the mirror's redirect target through it"
    );
    Ok(())
}
