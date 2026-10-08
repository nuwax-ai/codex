use super::HostBlockDecision;
use super::HostBlockReason;
use super::HostLookupFixture;
use super::network_proxy_state_for_policy;
use crate::config::NetworkProxyConfig;
use pretty_assertions::assert_eq;
use std::net::SocketAddr;
use std::sync::Arc;

#[tokio::test]
async fn host_policy_checks_every_fixture_address_family() {
    for (addresses, expected) in [
        (
            vec!["93.184.216.34:443", "[fd00::1]:443"],
            HostBlockDecision::Blocked(HostBlockReason::NotAllowedLocal),
        ),
        (
            vec!["10.0.0.1:443", "[2606:4700:4700::1111]:443"],
            HostBlockDecision::Blocked(HostBlockReason::NotAllowedLocal),
        ),
        (
            vec!["93.184.216.34:443", "[2606:4700:4700::1111]:443"],
            HostBlockDecision::Allowed,
        ),
    ] {
        let addresses: Vec<SocketAddr> = addresses
            .into_iter()
            .map(|address| address.parse().expect("valid fixture address"))
            .collect();
        let fixture: HostLookupFixture = Arc::new(move |host, port| {
            assert_eq!((host.as_str(), port), ("fixture.example", 443));
            let addresses = addresses.clone();
            Box::pin(async move { Ok(addresses) })
        });
        let mut config = NetworkProxyConfig::default();
        config.set_allowed_domains(vec!["fixture.example".to_string()]);
        let state = network_proxy_state_for_policy(config).with_host_lookup_fixture(fixture);

        assert_eq!(
            state
                .host_blocked("fixture.example", /*port*/ 443)
                .await
                .unwrap(),
            expected
        );
    }
}

#[cfg(target_os = "macos")]
mod native_connector {
    use super::*;
    use crate::connect_policy::StateDnsResolver;
    use crate::runtime::public_dns_lookup_fixture;
    use crate::system_dns::SystemDnsResolver;
    use pretty_assertions::assert_eq;
    use rama_dns::DnsResolver;
    use rama_net::address::Domain;
    use std::net::Ipv4Addr;
    use std::net::Ipv6Addr;
    use std::sync::Mutex;

    #[tokio::test]
    async fn fixture_resolves_both_address_families_without_system_dns() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let observed_calls = calls.clone();
        let addresses = vec![
            "93.184.216.34:0".parse::<SocketAddr>().unwrap(),
            "[fd00::1]:0".parse::<SocketAddr>().unwrap(),
        ];
        let fixture: HostLookupFixture = Arc::new(move |host, port| {
            observed_calls.lock().unwrap().push((host, port));
            let addresses = addresses.clone();
            Box::pin(async move { Ok(addresses) })
        });
        let resolver = StateDnsResolver::new(Some(fixture));
        let domain: Domain = "controlled.invalid".parse().unwrap();

        let (ipv4, ipv6) = tokio::join!(
            resolver.ipv4_lookup(domain.clone()),
            resolver.ipv6_lookup(domain),
        );

        assert_eq!(
            (ipv4.unwrap(), ipv6.unwrap()),
            (
                vec![Ipv4Addr::new(93, 184, 216, 34)],
                vec!["fd00::1".parse::<Ipv6Addr>().unwrap()],
            )
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![("controlled.invalid".to_string(), 0); 2]
        );
    }

    #[tokio::test]
    async fn public_fixture_has_no_system_ipv6_fallback() {
        let resolver = StateDnsResolver::new(Some(public_dns_lookup_fixture()));
        let domain: Domain = "public-fixture.example".parse().unwrap();
        let (ipv4, ipv6) = tokio::join!(
            resolver.ipv4_lookup(domain.clone()),
            resolver.ipv6_lookup(domain),
        );

        assert_eq!(
            (ipv4.unwrap(), ipv6.unwrap()),
            (vec![Ipv4Addr::new(93, 184, 216, 34)], Vec::new())
        );
    }

    #[tokio::test]
    async fn fixture_nxdomain_is_preserved_for_both_address_families() {
        let resolver = StateDnsResolver::new(Some(public_dns_lookup_fixture()));
        let domain: Domain = "does-not-resolve.invalid".parse().unwrap();
        let (ipv4, ipv6) = tokio::join!(
            resolver.ipv4_lookup(domain.clone()),
            resolver.ipv6_lookup(domain),
        );

        assert_eq!(
            (ipv4.unwrap_err().kind(), ipv6.unwrap_err().kind()),
            (std::io::ErrorKind::NotFound, std::io::ErrorKind::NotFound)
        );
    }

    #[tokio::test]
    async fn localhost_names_and_unconfigured_resolver_keep_native_resolution() {
        let fixture: HostLookupFixture = Arc::new(|host, _port| {
            panic!("localhost must bypass the fixture: {host}");
        });
        for resolver in [
            StateDnsResolver::new(/*fixture*/ None),
            StateDnsResolver::new(Some(fixture)),
        ] {
            for hostname in ["localhost", "localhost.", "LOCALHOST", "child.localhost"] {
                let domain: Domain = hostname.parse().unwrap();
                let expected = tokio::join!(
                    SystemDnsResolver.ipv4_lookup(domain.clone()),
                    SystemDnsResolver.ipv6_lookup(domain.clone()),
                );
                let actual = tokio::join!(
                    resolver.ipv4_lookup(domain.clone()),
                    resolver.ipv6_lookup(domain),
                );

                assert_eq!(
                    (
                        actual.0.map_err(|error| error.kind()),
                        actual.1.map_err(|error| error.kind())
                    ),
                    (
                        expected.0.map_err(|error| error.kind()),
                        expected.1.map_err(|error| error.kind())
                    ),
                    "native resolution for {hostname}"
                );
            }
        }
    }
}
