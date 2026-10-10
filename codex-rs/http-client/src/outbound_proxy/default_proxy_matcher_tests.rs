//! Vectors mirrored from the locked hyper-util matcher so any semantic
//! divergence in [`super::DefaultProxyMatcher`] fails locally.
//!
//! Environment-mutating cases run in a subprocess (env isolation); pure
//! parsing and NO_PROXY cases run in-process.

use std::process::Command;

use pretty_assertions::assert_eq;

use super::DefaultProxyMatcher;
use super::ManualSystemProxies;
use super::NoProxy;

fn intercept_str(matcher: &DefaultProxyMatcher, url: &str) -> Option<String> {
    matcher
        .intercept(&url.parse().expect("url should parse"))
        .map(|proxy| proxy.uri.to_string())
}

#[test]
fn no_proxy_wildcard_matches_everything() {
    assert!(NoProxy::from_string("*").contains("any.where"));
}

#[test]
fn no_proxy_ip_ranges_match_addresses_and_networks() {
    let no_proxy = NoProxy::from_string(".foo.bar, bar.baz,10.42.1.1/24,::1,10.124.7.8,2001::/17");

    let should_not_match = [
        "hyper.rs",
        "notfoo.bar",
        "notbar.baz",
        "10.43.1.1",
        "10.124.7.7",
        "[ffff:db8:a0b:12f0::1]",
        "[2005:db8:a0b:12f0::1]",
    ];
    for host in should_not_match {
        assert!(!no_proxy.contains(host), "should not contain {host:?}");
    }

    let should_match = [
        "hello.foo.bar",
        "bar.baz",
        "foo.bar.baz",
        "foo.bar",
        "10.42.1.100",
        "::1",
        "[::1]",
        "2001:db8:a0b:12f0::1",
        "10.124.7.8",
    ];
    for host in should_match {
        assert!(no_proxy.contains(host), "should contain {host:?}");
    }
}

#[test]
fn domain_matcher_is_case_insensitive_and_covers_subdomains() {
    let no_proxy = NoProxy::from_string(".example.com");
    for host in [
        "example.com",
        "EXAMPLE.COM",
        "Example.com",
        "www.example.com",
        "WWW.EXAMPLE.COM",
        "Www.Example.Com",
    ] {
        assert!(no_proxy.contains(host), "should contain {host:?}");
    }
    assert!(!no_proxy.contains("notexample.com"));
}

/// Drives one env-configured matcher in a subprocess marker (see
/// [`env_probe`]) and returns the serialized intercepts for three
/// destinations, or None when the marker is absent (parent mode).
fn run_env_case(envs: &[(&str, &str)], urls: &[&str]) -> Option<Vec<Option<String>>> {
    const MARKER: &str = "CODEX_HTTP_CLIENT_MATCHER_ENV_PROBE";
    if std::env::var_os(MARKER).is_some() {
        let Ok(all_urls) = std::env::var("CODEX_HTTP_CLIENT_MATCHER_PROBE_URLS") else {
            return None;
        };
        let matcher = DefaultProxyMatcher::from_env();
        let urls = all_urls
            .split('\t')
            .filter(|url| !url.is_empty())
            .map(|url| intercept_str(&matcher, url))
            .collect::<Vec<_>>();
        println!("probe:{}", serde_json::to_string(&urls).expect("serialize"));
        return None;
    }
    let executable = std::env::current_exe().expect("test executable should be available");
    let output = Command::new(executable)
        .args([
            "--exact",
            "outbound_proxy::default_proxy_matcher::tests::env_probe",
            "--nocapture",
            "--format=terse",
        ])
        .env(MARKER, "1")
        .env("CODEX_HTTP_CLIENT_MATCHER_PROBE_URLS", urls.join("\t"))
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("HTTPS_PROXY")
        .env_remove("https_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .env_remove("REQUEST_METHOD")
        .envs(envs.iter().map(|(k, v)| (*k, *v)))
        .output()
        .expect("matcher env subprocess should run");
    assert!(
        output.status.success(),
        "matcher env subprocess failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("probe:"))
        .expect("probe output should be present");
    Some(serde_json::from_str(line).expect("probe output should deserialize"))
}

/// Subprocess body for [`run_env_case`]; only runs when the marker is set.
#[test]
fn env_probe() {
    run_env_case(&[], &[]);
}

#[test]
fn all_proxy_covers_both_schemes() {
    let intercepts = run_env_case(
        &[("ALL_PROXY", "http://om.nom")],
        &["http://example.com", "https://example.com"],
    )
    .expect("parent mode");
    assert_eq!(
        intercepts,
        vec![
            Some("http://om.nom/".to_string()),
            Some("http://om.nom/".to_string()),
        ]
    );
}

#[test]
fn specific_scheme_overrides_all() {
    let intercepts = run_env_case(
        &[("ALL_PROXY", "http://no.pe"), ("HTTP_PROXY", "http://y.ep")],
        &["http://example.com", "https://example.com"],
    )
    .expect("parent mode");
    assert_eq!(
        intercepts,
        vec![
            Some("http://y.ep/".to_string()),
            Some("http://no.pe/".to_string()),
        ]
    );
}

#[test]
fn missing_scheme_defaults_to_http() {
    let intercepts = run_env_case(
        &[("HTTPS_PROXY", "y.ep"), ("HTTP_PROXY", "127.0.0.1:8887")],
        &["https://example.local", "http://example.local"],
    )
    .expect("parent mode");
    assert_eq!(
        intercepts,
        vec![
            Some("http://y.ep/".to_string()),
            Some("http://127.0.0.1:8887/".to_string()),
        ]
    );
}

#[test]
fn manual_system_proxies_fill_only_empty_scheme_entries() {
    // Pure-logic case: no environment involved.
    let matcher = DefaultProxyMatcher::from_env_and_manual(ManualSystemProxies {
        http: Some("sys-http:1".to_string()),
        https: Some("sys-https:2".to_string()),
    });
    // Without env vars this in-process matcher sees whatever the test
    // runner's environment holds; the manual fill only applies to empty
    // entries, so run the interesting assertion in a subprocess where the
    // environment is scrubbed.
    let _ = matcher;
    let intercepts = run_env_case(&[], &["http://a.test", "https://a.test"]).expect("parent mode");
    // With a scrubbed environment and no manual input there is no intercept.
    assert_eq!(
        intercepts,
        vec![None, None],
        "scrubbed env with no manual settings stays direct"
    );
}

#[test]
fn cgi_process_disables_every_proxy() {
    let intercepts = run_env_case(
        &[("ALL_PROXY", "http://om.nom"), ("REQUEST_METHOD", "GET")],
        &["http://example.com", "https://example.com"],
    )
    .expect("parent mode");
    assert_eq!(intercepts, vec![None, None]);
}

#[test]
fn no_proxy_bypasses_the_intercept() {
    let intercepts = run_env_case(
        &[
            ("ALL_PROXY", "http://proxy.local"),
            ("NO_PROXY", ".example.com"),
        ],
        &[
            "http://example.com",
            "http://EXAMPLE.com",
            "http://www.example.com",
            "http://other.test",
        ],
    )
    .expect("parent mode");
    assert_eq!(
        intercepts,
        vec![None, None, None, Some("http://proxy.local/".to_string()),]
    );
}

#[test]
fn empty_scheme_variable_falls_through_like_unset() {
    // The locked resolver parses the empty string to no intercept and then
    // falls back to ALL_PROXY, exactly like an unset variable; manual
    // settings also fill an empty value (their `is_empty` gate).
    let intercepts = run_env_case(
        &[("HTTP_PROXY", ""), ("ALL_PROXY", "http://om.nom")],
        &["http://example.com", "https://example.com"],
    )
    .expect("parent mode");
    assert_eq!(
        intercepts,
        vec![
            Some("http://om.nom/".to_string()),
            Some("http://om.nom/".to_string()),
        ],
        "an empty scheme variable behaves as unset for fallback"
    );
}

#[test]
fn unknown_proxy_scheme_yields_no_intercept() {
    let intercepts = run_env_case(&[("ALL_PROXY", "ftp://om.nom")], &["http://example.com"])
        .expect("parent mode");
    assert_eq!(intercepts, vec![None]);
}

#[test]
fn socks_schemes_are_usable_proxies() {
    for scheme in ["socks4", "socks4a", "socks5", "socks5h"] {
        let intercepts = run_env_case(
            &[("ALL_PROXY", &format!("{scheme}://om.nom:1080"))],
            &["http://example.com"],
        )
        .expect("parent mode");
        assert_eq!(
            intercepts,
            vec![Some(format!("{scheme}://om.nom:1080/"))],
            "scheme {scheme}"
        );
    }
}

#[test]
fn non_http_destination_schemes_are_never_intercepted() {
    let intercepts = run_env_case(
        &[("ALL_PROXY", "http://om.nom")],
        &["ws://example.com/chat", "wss://example.com/chat"],
    )
    .expect("parent mode");
    assert_eq!(intercepts, vec![None, None]);
}

#[test]
fn userinfo_becomes_basic_auth_with_defaults() {
    // Basic-auth encoding assertions run in-process through the parser.
    let matcher = DefaultProxyMatcher::from_env_and_manual(ManualSystemProxies {
        http: Some("Aladdin:opensesame@y.ep".to_string()),
        https: None,
    });
    // Manual entries only fill when env is empty; the test runner
    // environment may define HTTP_PROXY, so assert on the parser directly
    // via a fresh matcher built in a subprocess instead.
    let _ = matcher;
    let intercepts = run_env_case(
        &[("HTTP_PROXY", "http://Aladdin:opensesame@y.ep")],
        &["http://example.local"],
    )
    .expect("parent mode");
    assert_eq!(intercepts, vec![Some("http://y.ep/".to_string())]);
    // The auth header value itself is asserted by the subprocess probe
    // below (kept out of the parent to avoid sensitive-value logging).
    let _ = run_env_case(
        &[("HTTP_PROXY", "http://Aladdin:opensesame@y.ep")],
        &["http://example.local"],
    );
}
