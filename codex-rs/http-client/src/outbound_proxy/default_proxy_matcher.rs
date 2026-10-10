//! Default-mode proxy matcher replicating the locked transport's semantics.
//!
//! The default outbound policy must preserve reqwest/hyper-util's proxy
//! behavior exactly. That behavior (hyper-util 0.1.20, verified field by
//! field against its source) is:
//!
//! 1. environment first: `ALL_PROXY`/`all_proxy` for every scheme,
//!    `HTTP_PROXY`/`http_proxy` and `HTTPS_PROXY`/`https_proxy` per scheme
//!    (first variable that is *set* wins, even when its value is empty),
//!    `NO_PROXY`/`no_proxy` as a curl-style exclusion list;
//! 2. manual system settings fill only the scheme values the environment
//!    left empty (macOS: the dynamic-store `HTTPEnable`/`HTTPSEnable` manual
//!    entries; the system exclusion list is NOT consulted);
//! 3. a CGI process (`REQUEST_METHOD` set) disables every proxy;
//! 4. per-destination: NO_PROXY wins over everything, then the scheme entry
//!    falls back to ALL_PROXY; proxy URLs without a scheme mean `http`;
//!    userinfo is percent-decoded and becomes basic auth on http(s) proxies;
//!    only `http`, `https`, `socks4`, `socks4a`, `socks5`, `socks5h` schemes
//!    are usable;
//! 5. destinations whose scheme is neither `http` nor `https` are never
//!    intercepted (ws/wss are not proxied by the default matcher).
//!
//! This module is the shared decision core; per-platform adapters feed it
//! environment snapshots and manual system settings. Tests mirror the
//! upstream matcher's own vectors so any divergence is caught locally.

use std::net::IpAddr;

use base64::Engine as _;
use base64::prelude::BASE64_STANDARD;
use http::Uri;
use http::header::HeaderValue;

/// Manual system proxy entries used to fill scheme values the environment
/// left empty (`host:port` or bare `host`, exactly as the dynamic store
/// reader emits them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
// Wired by the default-mode takeover (T07 stage 3); the matcher core and its
// parity vectors land first so the takeover is diffable on its own.
#[allow(dead_code)]
pub(crate) struct ManualSystemProxies {
    pub(crate) http: Option<String>,
    pub(crate) https: Option<String>,
}

/// A matched proxy destination plus its parsed authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct DefaultProxyIntercept {
    pub(crate) uri: Uri,
    pub(crate) basic_auth: Option<HeaderValue>,
}

#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub(crate) struct DefaultProxyMatcher {
    http: Option<DefaultProxyIntercept>,
    https: Option<DefaultProxyIntercept>,
    no: NoProxy,
}

impl DefaultProxyMatcher {
    /// Builds the matcher from the process environment alone.
    pub(crate) fn from_env() -> Self {
        Self::from_env_and_manual(ManualSystemProxies::default())
    }

    /// Builds the matcher, letting `manual` fill only the scheme entries the
    /// environment left empty — the locked resolver's precedence.
    pub(crate) fn from_env_and_manual(manual: ManualSystemProxies) -> Self {
        let is_cgi = std::env::var_os("REQUEST_METHOD").is_some();
        if is_cgi {
            return Self::default();
        }
        let mut http = env_first(&["HTTP_PROXY", "http_proxy"]);
        let mut https = env_first(&["HTTPS_PROXY", "https_proxy"]);
        let all = env_first(&["ALL_PROXY", "all_proxy"]);
        // The locked resolver gates manual fill on the string being EMPTY,
        // so an explicitly empty scheme variable is treated as unset here.
        if http.as_deref().unwrap_or("").is_empty() {
            http = manual.http;
        }
        if https.as_deref().unwrap_or("").is_empty() {
            https = manual.https;
        }
        let no = env_first(&["NO_PROXY", "no_proxy"]).unwrap_or_default();
        let all_intercept = all.as_deref().and_then(parse_proxy_uri);
        Self {
            http: http
                .as_deref()
                .and_then(parse_proxy_uri)
                .or(all_intercept.clone()),
            https: https.as_deref().and_then(parse_proxy_uri).or(all_intercept),
            no: NoProxy::from_string(&no),
        }
    }

    /// Returns the intercept for `dst`, or `None` to connect directly.
    pub(crate) fn intercept(&self, dst: &Uri) -> Option<&DefaultProxyIntercept> {
        if self.no.contains(dst.host()?) {
            return None;
        }
        match dst.scheme_str() {
            Some("http") => self.http.as_ref(),
            Some("https") => self.https.as_ref(),
            _ => None,
        }
    }
}

/// First set variable among `keys` (empty values count as set, matching the
/// locked resolver's `get_first_env`).
fn env_first(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| std::env::var(key).ok())
}

/// Parses one proxy URL per the locked `parse_env_uri`: authority required,
/// missing scheme means `http`, percent-decoded userinfo becomes basic auth
/// on http(s) proxies, and unknown schemes yield no intercept.
fn parse_proxy_uri(value: &str) -> Option<DefaultProxyIntercept> {
    let uri = value.parse::<Uri>().ok()?;
    let mut builder = Uri::builder();
    let mut is_httpish = false;
    builder = builder.scheme(match uri.scheme() {
        Some(s) => {
            if s == &http::uri::Scheme::HTTP || s == &http::uri::Scheme::HTTPS {
                is_httpish = true;
                s.clone()
            } else if matches!(s.as_str(), "socks4" | "socks4a" | "socks5" | "socks5h") {
                s.clone()
            } else {
                // Unusable proxy scheme: behave as if unset.
                return None;
            }
        }
        None => {
            is_httpish = true;
            http::uri::Scheme::HTTP
        }
    });

    let raw_authority = uri.authority()?;
    let mut basic_auth = None;
    let authority = if let Some((userinfo, host_port)) = raw_authority.as_str().split_once('@') {
        let (user, pass) = match userinfo.split_once(':') {
            Some((user, pass)) => (user, Some(pass)),
            None => (userinfo, None),
        };
        let user = percent_decode_str(user);
        let pass = pass.map(percent_decode_str);
        if is_httpish {
            basic_auth = Some(encode_basic_auth(&user, pass.as_deref()));
        }
        host_port
    } else {
        raw_authority.as_str()
    };
    builder = builder.authority(authority);
    // A path is required or the builder errors, exactly like the reference.
    let uri = builder.path_and_query("/").build().ok()?;
    Some(DefaultProxyIntercept { uri, basic_auth })
}

/// Percent-decoding exactly per the locked `percent_decode_str`: `%` must be
/// followed by two hex digits, and the decoded bytes become lossy UTF-8.
fn percent_decode_str(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
        {
            output.push((hi << 4) | lo);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn encode_basic_auth(user: &str, pass: Option<&str>) -> HeaderValue {
    let mut buf = b"Basic ".to_vec();
    {
        use std::io::Write;
        let mut encoder = base64::write::EncoderWriter::new(&mut buf, &BASE64_STANDARD);
        let _ = write!(encoder, "{user}:");
        if let Some(password) = pass {
            let _ = write!(encoder, "{password}");
        }
    }
    let mut header = HeaderValue::from_bytes(&buf).expect("base64 is always valid HeaderValue");
    header.set_sensitive(true);
    header
}

/// curl-style NO_PROXY list: `*` matches everything; IP addresses match
/// exactly or by CIDR; anything else is a domain that also matches its
/// subdomains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct NoProxy {
    ips: Vec<IpRule>,
    domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum IpRule {
    Address(IpAddr),
    Network(IpNet),
}

/// Minimal CIDR network with ipnet's semantics: host bits in the address are
/// allowed at parse time and ignored for containment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IpNet {
    V4 { addr: [u8; 4], prefix: u8 },
    V6 { addr: [u16; 8], prefix: u8 },
}

impl IpNet {
    fn contains(&self, addr: &IpAddr) -> bool {
        match (self, addr) {
            (IpNet::V4 { addr, prefix }, IpAddr::V4(other)) => mask_v4(*prefix)
                .iter()
                .zip(addr.iter().zip(other.octets().iter()))
                .all(|(m, (a, b))| a & m == b & m),
            (IpNet::V6 { addr, prefix }, IpAddr::V6(other)) => {
                let segments = other.segments();
                let mut matched = true;
                for (index, (a, b)) in addr.iter().zip(segments.iter()).enumerate() {
                    let shift = segment_shift(index, *prefix);
                    if shift == 0 {
                        continue;
                    }
                    let mask: u16 = if shift >= 16 {
                        u16::MAX
                    } else {
                        u16::MAX << (16 - shift)
                    };
                    if a & mask != b & mask {
                        matched = false;
                        break;
                    }
                }
                matched
            }
            _ => false,
        }
    }
}

fn mask_v4(prefix: u8) -> [u8; 4] {
    let bits = u32::from(prefix);
    let mask = if bits == 0 {
        0
    } else {
        u32::MAX << (32 - bits)
    };
    mask.to_be_bytes()
}

fn segment_shift(segment_index: usize, prefix: u8) -> u8 {
    prefix
        .saturating_sub((segment_index as u16 * 16) as u8)
        .min(16)
}

impl NoProxy {
    /// Parses a NO_PROXY value: comma-separated entries, whitespace-trimmed;
    /// CIDR/IP entries match addresses; `*` matches every host; other entries
    /// are domains and also match their subdomains.
    pub(crate) fn from_string(no_proxy_list: &str) -> Self {
        let mut ips = Vec::new();
        let mut domains = Vec::new();
        for part in no_proxy_list.split(',').map(str::trim) {
            if let Some(rule) = parse_ip_rule(part) {
                ips.push(rule);
                continue;
            }
            if !part.is_empty() {
                domains.push(part.to_owned());
            }
        }
        Self { ips, domains }
    }

    /// True when `host` is bypassed. Raw IPv6 hosts arrive bracketed.
    pub(crate) fn contains(&self, host: &str) -> bool {
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        match host.parse::<IpAddr>() {
            Ok(ip) => self.ips.iter().any(|rule| match rule {
                IpRule::Address(address) => address == &ip,
                IpRule::Network(net) => net.contains(&ip),
            }),
            Err(_) => self.domains.iter().any(|d| domain_matches(d, host)),
        }
    }

    fn is_empty(&self) -> bool {
        self.ips.is_empty() && self.domains.is_empty()
    }
}

fn parse_ip_rule(part: &str) -> Option<IpRule> {
    if let Some(addr) = part.parse::<IpAddr>().ok() {
        return Some(IpRule::Address(addr));
    }
    let (addr, prefix) = part.split_once('/')?;
    let prefix: u8 = prefix.parse().ok()?;
    match addr.parse::<std::net::Ipv4Addr>() {
        Ok(v4) if prefix <= 32 => Some(IpRule::Network(IpNet::V4 {
            addr: v4.octets(),
            prefix,
        })),
        _ => match addr.parse::<std::net::Ipv6Addr>() {
            Ok(v6) if prefix <= 128 => {
                let mut segments = [0u16; 8];
                for (slot, seg) in segments.iter_mut().zip(v6.segments()) {
                    *slot = seg;
                }
                Some(IpRule::Network(IpNet::V6 {
                    addr: segments,
                    prefix,
                }))
            }
            _ => None,
        },
    }
}

/// Domain matching per the locked `DomainMatcher`: case-insensitive exact
/// match, leading-dot equivalence, and suffix matches only across a dot
/// boundary; `*` matches everything.
fn domain_matches(d: &str, host: &str) -> bool {
    if d == "*" {
        return true;
    }
    if d.eq_ignore_ascii_case(host) {
        return true;
    }
    let stripped = d.strip_prefix('.').unwrap_or(d);
    if stripped.eq_ignore_ascii_case(host) {
        return true;
    }
    if host.len() >= d.len() && host[host.len() - d.len()..].eq_ignore_ascii_case(d) {
        if d.starts_with('.') {
            return true;
        }
        if host.as_bytes().get(host.len() - d.len() - 1) == Some(&b'.') {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[path = "default_proxy_matcher_tests.rs"]
mod tests;
