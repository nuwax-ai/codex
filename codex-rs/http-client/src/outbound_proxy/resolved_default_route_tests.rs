//! Route-level vectors for the opt-in ResolvedDefault policy.
//!
//! Each case pins the route [`super::resolve_proxy_route`] produces for one
//! environment shape. Wire-level equivalence against a reference default
//! reqwest client lives in `resolved_default_dual_track_tests.rs`.

use super::*;
use pretty_assertions::assert_eq;

struct MapEnv {
    values: std::collections::HashMap<String, String>,
}

impl EnvSource for MapEnv {
    fn var(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
}

fn env(pairs: &[(&str, &str)]) -> MapEnv {
    MapEnv {
        values: pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

fn route(pairs: &[(&str, &str)], url: &str) -> OutboundProxyRoute {
    resolve_proxy_route(
        &env(pairs),
        url,
        OutboundProxyPolicy::ResolvedDefault,
        |_, _| unreachable!("ResolvedDefault never consults the system resolver"),
    )
}

#[test]
fn resolved_default_maps_environment_proxies_per_scheme() {
    // Specific scheme entries win; ALL_PROXY is the fallback.
    assert_eq!(
        route(&[("HTTP_PROXY", "http://p1:1")], "http://a.test/x"),
        OutboundProxyRoute::Proxy {
            url: "http://p1:1".to_string(),
            no_proxy: None,
        }
    );
    assert_eq!(
        route(&[("HTTPS_PROXY", "http://p2:2")], "https://a.test/x"),
        OutboundProxyRoute::Proxy {
            url: "http://p2:2".to_string(),
            no_proxy: None,
        }
    );
    assert_eq!(
        route(&[("ALL_PROXY", "http://p3:3")], "https://a.test/x"),
        OutboundProxyRoute::Proxy {
            url: "http://p3:3".to_string(),
            no_proxy: None,
        }
    );
    // http destinations fall back to ALL_PROXY only without HTTP_PROXY.
    assert_eq!(
        route(
            &[("HTTP_PROXY", "http://p1:1"), ("ALL_PROXY", "http://p3:3")],
            "http://a.test/x"
        ),
        OutboundProxyRoute::Proxy {
            url: "http://p1:1".to_string(),
            no_proxy: None,
        }
    );
}

#[test]
fn resolved_default_applies_no_proxy_first() {
    assert_eq!(
        route(
            &[("ALL_PROXY", "http://p3:3"), ("NO_PROXY", ".a.test"),],
            "https://www.a.test/x"
        ),
        OutboundProxyRoute::Direct
    );
    assert_eq!(
        route(
            &[("ALL_PROXY", "http://p3:3"), ("NO_PROXY", ".a.test"),],
            "https://other.test/x"
        ),
        OutboundProxyRoute::Proxy {
            url: "http://p3:3".to_string(),
            no_proxy: None,
        }
    );
}

#[test]
fn resolved_default_covers_literal_ips_and_localhost() {
    // The default matcher has no implicit local bypass: literal IPs and
    // localhost route through the environment proxy like any other host.
    for url in [
        "http://127.0.0.1:8080/x",
        "http://localhost:3000/x",
        "http://192.0.2.10/v1",
        "https://[2001:db8::1]/v1",
    ] {
        assert_eq!(
            route(
                &[
                    ("HTTP_PROXY", "http://p1:1"),
                    ("HTTPS_PROXY", "http://p2:2")
                ],
                url
            ),
            OutboundProxyRoute::Proxy {
                url: if url.starts_with("https") {
                    "http://p2:2".to_string()
                } else {
                    "http://p1:1".to_string()
                },
                no_proxy: None,
            },
            "literal destination {url} must ride the environment proxy"
        );
    }
}

#[test]
fn resolved_default_direct_without_any_proxy_configuration() {
    assert_eq!(route(&[], "https://a.test/x"), OutboundProxyRoute::Direct);
    // A wildcard NO_PROXY is also direct.
    assert_eq!(
        route(
            &[("ALL_PROXY", "http://p3:3"), ("NO_PROXY", "*")],
            "https://a.test/x"
        ),
        OutboundProxyRoute::Direct
    );
}

#[test]
fn resolved_default_delegates_socks_schemes_to_the_transport() {
    for scheme in ["socks4", "socks4a", "socks5", "socks5h"] {
        assert_eq!(
            route(
                &[("ALL_PROXY", &format!("{scheme}://p5:1080"))],
                "http://a.test/x"
            ),
            OutboundProxyRoute::TransportDefault,
            "{scheme} proxies keep the transport default for exact parity"
        );
    }
}

#[test]
fn resolved_default_preserves_proxy_userinfo_in_the_route() {
    // OutboundProxyRoute redacts Proxy fields in Debug/Display; compare the
    // URL directly.
    let OutboundProxyRoute::Proxy { url, .. } = route(
        &[("HTTP_PROXY", "http://user:secret@p1:1")],
        "http://a.test/x",
    ) else {
        panic!("userinfo proxies must map to an explicit proxy route");
    };
    assert_eq!(url, "http://user:secret@p1:1");
}

#[test]
fn resolved_default_never_proxies_websocket_schemes() {
    // The default matcher only intercepts http/https destinations.
    assert_eq!(
        route(&[("ALL_PROXY", "http://p3:3")], "wss://a.test/chat"),
        OutboundProxyRoute::Direct
    );
}
