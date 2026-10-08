use crate::policy::is_non_public_ip;
use crate::runtime::HostBlockDecision;
use crate::state::NetworkProxyState;
#[cfg(target_os = "macos")]
use crate::system_dns::SystemDnsResolver;
use rama_core::Service;
use rama_core::error::BoxError;
use rama_core::error::ErrorExt as _;
use rama_core::error::OpaqueError;
use rama_core::extensions::ExtensionsMut;
#[cfg(target_os = "macos")]
use rama_dns::DnsResolver;
#[cfg(target_os = "macos")]
use rama_net::address::Domain;
use rama_net::address::Host;
use rama_net::address::HostWithPort;
use rama_net::address::ProxyAddress;
use rama_net::client::EstablishedClientConnection;
use rama_net::transport::TryRefIntoTransportContext;
use rama_tcp::TcpStream;
use rama_tcp::client::TcpStreamConnector;
use rama_tcp::client::service::TcpConnector;
use std::io;
#[cfg(target_os = "macos")]
use std::net::IpAddr;
#[cfg(target_os = "macos")]
use std::net::Ipv4Addr;
#[cfg(target_os = "macos")]
use std::net::Ipv6Addr;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct TargetCheckedTcpConnector {
    state: Arc<NetworkProxyState>,
}

impl TargetCheckedTcpConnector {
    pub(crate) fn new(state: Arc<NetworkProxyState>) -> Self {
        Self { state }
    }
}

/// DNS resolver for the target-checked connector: consults the controlled
/// test fixture when one is installed, otherwise the native system resolver.
#[cfg(target_os = "macos")]
#[derive(Clone)]
pub(crate) struct StateDnsResolver {
    fixture: Option<crate::runtime::HostLookupFixture>,
}

#[cfg(target_os = "macos")]
impl StateDnsResolver {
    pub(crate) fn new(fixture: Option<crate::runtime::HostLookupFixture>) -> Self {
        Self { fixture }
    }

    /// Loopback names always use the native resolver: connector-level IPv4 and
    /// IPv6 localhost tests must observe both real address families.
    fn fixture_for_host(&self, host: &str) -> Option<&crate::runtime::HostLookupFixture> {
        let host = crate::policy::normalize_host(host);
        if host == "localhost" || host.ends_with(".localhost") {
            None
        } else {
            self.fixture.as_ref()
        }
    }
}

#[cfg(target_os = "macos")]
impl DnsResolver for StateDnsResolver {
    type Error = std::io::Error;

    async fn ipv4_lookup(&self, domain: Domain) -> std::io::Result<Vec<Ipv4Addr>> {
        if let Some(fixture) = self.fixture_for_host(domain.as_str()) {
            let addrs = fixture(domain.as_str().to_owned(), /*port*/ 0).await?;
            return Ok(addrs
                .into_iter()
                .filter_map(|addr| match addr.ip() {
                    IpAddr::V4(ip) => Some(ip),
                    IpAddr::V6(_) => None,
                })
                .collect());
        }
        SystemDnsResolver.ipv4_lookup(domain).await
    }

    async fn ipv6_lookup(&self, domain: Domain) -> std::io::Result<Vec<Ipv6Addr>> {
        if let Some(fixture) = self.fixture_for_host(domain.as_str()) {
            let addrs = fixture(domain.as_str().to_owned(), /*port*/ 0).await?;
            return Ok(addrs
                .into_iter()
                .filter_map(|addr| match addr.ip() {
                    IpAddr::V4(_) => None,
                    IpAddr::V6(ip) => Some(ip),
                })
                .collect());
        }
        SystemDnsResolver.ipv6_lookup(domain).await
    }

    async fn txt_lookup(&self, domain: Domain) -> std::io::Result<Vec<Vec<u8>>> {
        SystemDnsResolver.txt_lookup(domain).await
    }
}

impl<Input> Service<Input> for TargetCheckedTcpConnector
where
    Input: TryRefIntoTransportContext + Send + ExtensionsMut + 'static,
    Input::Error: Into<BoxError> + Send + Sync + 'static,
{
    type Output = EstablishedClientConnection<TcpStream, Input>;
    type Error = BoxError;

    async fn serve(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let connector = TcpConnector::new();
        #[cfg(target_os = "macos")]
        let connector = connector.with_dns(StateDnsResolver::new(
            self.state.host_lookup_fixture.clone(),
        ));

        if input.extensions().get::<ProxyAddress>().is_some() {
            return connector.serve(input).await;
        }

        let target = input
            .try_ref_into_transport_ctx()
            .map_err(|err| OpaqueError::from_boxed(err.into()).context("read network target"))?
            .host_with_port()
            .ok_or_else(|| OpaqueError::from_display("network target is missing a port"))?;

        connector
            .with_connector(TargetCheckedStreamConnector {
                state: self.state.clone(),
                target,
            })
            .serve(input)
            .await
    }
}

#[derive(Clone)]
struct TargetCheckedStreamConnector {
    state: Arc<NetworkProxyState>,
    target: HostWithPort,
}

impl TcpStreamConnector for TargetCheckedStreamConnector {
    type Error = BoxError;

    async fn connect(&self, addr: SocketAddr) -> Result<TcpStream, Self::Error> {
        if is_non_public_ip(addr.ip()) && !self.allows_non_public_target(addr).await? {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "network target rejected by policy",
            )
            .into());
        }

        tokio::net::TcpStream::connect(addr)
            .await
            .map(TcpStream::from)
            .map_err(Into::into)
    }
}

impl TargetCheckedStreamConnector {
    async fn allows_non_public_target(&self, addr: SocketAddr) -> Result<bool, BoxError> {
        if self.state.allow_local_binding().await.map_err(|err| {
            let err: BoxError = err.into();
            OpaqueError::from_boxed(err)
                .context("read network proxy config")
                .into_boxed()
        })? {
            return Ok(true);
        }

        if !target_matches_non_public_addr(&self.target.host, addr.ip()) {
            return Ok(false);
        }

        self.state
            .host_blocked(&self.target.host.to_string(), self.target.port)
            .await
            .map(|decision| decision == HostBlockDecision::Allowed)
            .map_err(|err| {
                let err: BoxError = err.into();
                OpaqueError::from_boxed(err)
                    .context("evaluate network proxy target")
                    .into_boxed()
            })
    }
}

pub(crate) fn is_non_public_target(host: &Host) -> bool {
    match host {
        Host::Address(ip) => is_non_public_ip(*ip),
        Host::Name(name) => name
            .as_str()
            .trim_end_matches('.')
            .eq_ignore_ascii_case("localhost"),
    }
}

fn target_matches_non_public_addr(host: &Host, addr: std::net::IpAddr) -> bool {
    match host {
        Host::Address(ip) => *ip == addr,
        Host::Name(name) => {
            name.as_str()
                .trim_end_matches('.')
                .eq_ignore_ascii_case("localhost")
                && addr.is_loopback()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NetworkProxyConfig;
    use crate::state::network_proxy_state_for_policy;
    use rama_net::address::HostWithPort;
    use std::net::Ipv4Addr;
    use tokio::net::TcpListener;

    #[tokio::test(flavor = "current_thread")]
    async fn direct_connector_rejects_non_public_target_when_local_binding_disabled() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local listener");
        let target = listener.local_addr().expect("local addr");
        let connector = TargetCheckedTcpConnector::new(Arc::new(network_proxy_state_for_policy(
            NetworkProxyConfig::default(),
        )));

        let request: rama_tcp::client::Request =
            rama_tcp::client::Request::new(HostWithPort::from(target));
        let err = Service::serve(&connector, request)
            .await
            .expect_err("local target should be rejected");

        assert!(
            format!("{err:?}").contains("network target rejected by policy"),
            "unexpected error: {err:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_connector_allows_non_public_target_when_local_binding_enabled() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local listener");
        let target = listener.local_addr().expect("local addr");
        let connector = TargetCheckedTcpConnector::new(Arc::new(network_proxy_state_for_policy(
            NetworkProxyConfig {
                allow_local_binding: Some(true),
                ..NetworkProxyConfig::default()
            },
        )));

        let request: rama_tcp::client::Request =
            rama_tcp::client::Request::new(HostWithPort::from(target));
        let result = Service::serve(&connector, request).await;

        assert!(result.is_ok(), "local target should be allowed: {result:?}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_connector_allows_explicitly_allowlisted_non_public_target() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local listener");
        let target = listener.local_addr().expect("local addr");
        let mut config = NetworkProxyConfig::default();
        config.set_allowed_domains(vec![target.ip().to_string()]);
        let connector =
            TargetCheckedTcpConnector::new(Arc::new(network_proxy_state_for_policy(config)));

        let request: rama_tcp::client::Request =
            rama_tcp::client::Request::new(HostWithPort::from(target));
        let result = Service::serve(&connector, request).await;

        assert!(
            result.is_ok(),
            "explicitly allowlisted local target should be allowed: {result:?}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_connector_allows_explicitly_allowlisted_localhost_target() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind local listener");
        let target = listener.local_addr().expect("local addr");
        let mut config = NetworkProxyConfig::default();
        config.set_allowed_domains(vec!["localhost".to_string()]);
        let connector =
            TargetCheckedTcpConnector::new(Arc::new(network_proxy_state_for_policy(config)));

        let request: rama_tcp::client::Request =
            rama_tcp::client::Request::new(HostWithPort::new(Host::LOCALHOST_NAME, target.port()));
        let result = Service::serve(&connector, request).await;

        assert!(
            result.is_ok(),
            "explicitly allowlisted localhost target should be allowed: {result:?}"
        );
    }

    #[test]
    fn resolved_private_address_does_not_match_allowlisted_hostname() {
        let host = Host::Name("example.com".parse().expect("valid domain"));

        assert!(!target_matches_non_public_addr(
            &host,
            Ipv4Addr::LOCALHOST.into()
        ));
    }
}
