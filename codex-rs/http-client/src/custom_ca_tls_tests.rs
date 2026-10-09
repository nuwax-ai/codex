//! Handshake coverage for the custom trust path retained by Windows realtime connections.

use super::CODEX_CA_CERT_ENV;
use super::ConfiguredCaBundle;
use super::EnvSource;
use super::NATIVE_ROOTS_CACHE_TTL;
use super::build_rustls_client_config;
use super::build_rustls_client_config_with_native_roots;
use super::with_native_roots_cache;
use pretty_assertions::assert_eq;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::CertifiedIssuer;
use rcgen::IsCa;
use rcgen::KeyPair;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

#[test]
fn custom_intermediate_trust_preserves_hostname_validation() {
    // The in-memory handshake helper does not touch the native-roots cache,
    // but the bundle build underneath loads platform roots; serialize against
    // the cache-semantics tests that swap fixture stores in.
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let root = CertifiedIssuer::self_signed(params.clone(), KeyPair::generate().unwrap()).unwrap();
    let intermediate =
        CertifiedIssuer::signed_by(params, KeyPair::generate().unwrap(), &root).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &intermediate)
        .unwrap();
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("intermediate.pem");
    std::fs::write(&path, intermediate.pem()).unwrap();
    let config = with_native_roots_cache(|| {
        build_rustls_client_config(Some(&ConfiguredCaBundle {
            source_env: CODEX_CA_CERT_ENV,
            path,
        }))
        .unwrap()
    });
    let server = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone()], leaf_key.into())
            .unwrap(),
    );

    assert_eq!(
        handshake(config.clone(), server.clone(), "localhost"),
        Ok(())
    );
    let error = handshake(config, server, "wrong.example").unwrap_err();
    assert!(
        matches!(
            error,
            rustls::Error::InvalidCertificate(
                rustls::CertificateError::NotValidForName
                    | rustls::CertificateError::NotValidForNameContext { .. }
            )
        ),
        "{error:?}"
    );
}

struct MapEnv {
    values: HashMap<String, String>,
}

impl EnvSource for MapEnv {
    fn var(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
}

struct TrustChain {
    issuer_pem: String,
    issuer_der: CertificateDer<'static>,
    server: Arc<rustls::ServerConfig>,
}

fn trust_chain(common_name: &str) -> TrustChain {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let mut distinguished_name = rcgen::DistinguishedName::new();
    distinguished_name.push(rcgen::DnType::CommonName, common_name);
    params.distinguished_name = distinguished_name;
    let issuer = CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&leaf_key, &issuer)
        .unwrap();
    let server = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone()], leaf_key.into())
            .unwrap(),
    );
    TrustChain {
        issuer_pem: issuer.pem(),
        issuer_der: issuer.der().clone(),
        server,
    }
}

fn pem_loader(path: &Path) -> impl FnOnce() -> rustls_native_certs::CertificateResult {
    let path = path.to_path_buf();
    move || match CertificateDer::pem_file_iter(&path) {
        Ok(iter) => {
            let mut certs = Vec::new();
            let mut errors = Vec::new();
            for cert in iter {
                match cert {
                    Ok(cert) => certs.push(cert),
                    Err(error) => errors.push(rustls_native_certs::Error {
                        context: "test root bundle",
                        kind: rustls_native_certs::ErrorKind::Pem(error),
                    }),
                }
            }
            {
                // CertificateResult is non-exhaustive, so build it from Default.
                let mut result = rustls_native_certs::CertificateResult::default();
                result.certs = certs;
                result.errors = errors;
                result
            }
        }
        Err(_) => rustls_native_certs::CertificateResult::default(),
    }
}

/// Replacing the bundle file under the same source must refresh trust for
/// connectors built after the TTL: the new root validates, the old one is
/// rejected, all within one process.
#[test]
fn native_roots_refresh_replaces_trust_for_new_connectors_in_the_same_process() {
    codex_utils_rustls_provider::ensure_rustls_crypto_provider();
    with_native_roots_cache(|| {
        let chain_a = trust_chain("refresh-roots-a");
        let chain_b = trust_chain("refresh-roots-b");
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("roots.pem");
        std::fs::write(&path, chain_a.issuer_pem).unwrap();
        let env = MapEnv {
            values: HashMap::from([(
                "SSL_CERT_FILE".to_string(),
                path.to_string_lossy().into_owned(),
            )]),
        };
        let now = Instant::now();

        let before = build_rustls_client_config_with_native_roots(
            &env,
            pem_loader(&path),
            now,
            /*bundle*/ None,
        )
        .unwrap();
        assert_eq!(
            handshake(before.clone(), chain_a.server.clone(), "localhost"),
            Ok(())
        );
        assert!(handshake(before, chain_b.server.clone(), "localhost").is_err());

        std::fs::write(&path, chain_b.issuer_pem).unwrap();
        let after = build_rustls_client_config_with_native_roots(
            &env,
            pem_loader(&path),
            now + NATIVE_ROOTS_CACHE_TTL + Duration::from_secs(/*secs*/ 1),
            /*bundle*/ None,
        )
        .unwrap();
        assert_eq!(
            handshake(after.clone(), chain_b.server, "localhost"),
            Ok(())
        );
        let error = handshake(after, chain_a.server, "localhost").unwrap_err();
        assert!(
            matches!(error, rustls::Error::InvalidCertificate(_)),
            "the replaced root must no longer anchor new connectors: {error:?}"
        );
    });
}

/// On Unix the loader itself honors `SSL_CERT_FILE`, so pointing the
/// environment at a different bundle must invalidate the cache immediately,
/// without waiting for the TTL.
#[cfg(unix)]
#[test]
fn native_roots_source_environment_change_invalidates_without_waiting_for_the_ttl() {
    codex_utils_rustls_provider::ensure_rustls_crypto_provider();
    with_native_roots_cache(|| {
        let chain_a = trust_chain("refresh-roots-a");
        let chain_b = trust_chain("refresh-roots-b");
        let temp = tempfile::TempDir::new().unwrap();
        let path_a = temp.path().join("roots-a.pem");
        let path_b = temp.path().join("roots-b.pem");
        std::fs::write(&path_a, chain_a.issuer_pem).unwrap();
        std::fs::write(&path_b, chain_b.issuer_pem).unwrap();
        let env_a = MapEnv {
            values: HashMap::from([(
                "SSL_CERT_FILE".to_string(),
                path_a.to_string_lossy().into_owned(),
            )]),
        };
        let env_b = MapEnv {
            values: HashMap::from([(
                "SSL_CERT_FILE".to_string(),
                path_b.to_string_lossy().into_owned(),
            )]),
        };
        let now = Instant::now();

        let from_a = build_rustls_client_config_with_native_roots(
            &env_a,
            pem_loader(&path_a),
            now,
            /*bundle*/ None,
        )
        .unwrap();
        assert_eq!(
            handshake(from_a, chain_a.server.clone(), "localhost"),
            Ok(())
        );
        let from_b = build_rustls_client_config_with_native_roots(
            &env_b,
            pem_loader(&path_b),
            now,
            /*bundle*/ None,
        )
        .unwrap();
        assert_eq!(
            handshake(from_b.clone(), chain_b.server, "localhost"),
            Ok(())
        );
        assert!(
            handshake(from_b, chain_a.server, "localhost").is_err(),
            "switching SSL_CERT_FILE must not keep trusting the previous source"
        );
    });
}

/// A configured custom bundle extends only the connectors that asked for it;
/// the cached platform roots must not accumulate bundle certificates.
#[test]
fn custom_bundle_certs_do_not_leak_into_the_cached_native_roots() {
    codex_utils_rustls_provider::ensure_rustls_crypto_provider();
    with_native_roots_cache(|| {
        let native = trust_chain("native-platform-root");
        let extra = trust_chain("extra-bundle-root");
        let temp = tempfile::TempDir::new().unwrap();
        let bundle_path = temp.path().join("bundle.pem");
        std::fs::write(&bundle_path, extra.issuer_pem).unwrap();
        let loader = || {
            {
                // CertificateResult is non-exhaustive, so build it from Default.
                let mut result = rustls_native_certs::CertificateResult::default();
                result.certs = vec![native.issuer_der.clone()];
                result
            }
        };
        let now = Instant::now();

        let with_bundle = build_rustls_client_config_with_native_roots(
            &MapEnv {
                values: HashMap::new(),
            },
            loader,
            now,
            Some(&ConfiguredCaBundle {
                source_env: CODEX_CA_CERT_ENV,
                path: bundle_path,
            }),
        )
        .unwrap();
        assert_eq!(
            handshake(with_bundle, extra.server.clone(), "localhost"),
            Ok(()),
            "the configured bundle must extend this connector's trust"
        );

        let without_bundle = build_rustls_client_config_with_native_roots(
            &MapEnv {
                values: HashMap::new(),
            },
            loader,
            now,
            /*bundle*/ None,
        )
        .unwrap();
        assert_eq!(
            handshake(without_bundle.clone(), native.server, "localhost"),
            Ok(())
        );
        assert!(
            handshake(without_bundle, extra.server, "localhost").is_err(),
            "bundle certificates must not pollute the cached platform roots"
        );
    });
}

fn handshake(
    config: Arc<rustls::ClientConfig>,
    server: Arc<rustls::ServerConfig>,
    hostname: &'static str,
) -> Result<(), rustls::Error> {
    let mut client = rustls::ClientConnection::new(config, hostname.try_into().unwrap()).unwrap();
    let mut server = rustls::ServerConnection::new(server).unwrap();
    for _ in 0..10 {
        let mut bytes = Vec::new();
        client.write_tls(&mut bytes).unwrap();
        server.read_tls(&mut bytes.as_slice()).unwrap();
        server.process_new_packets()?;
        bytes.clear();
        server.write_tls(&mut bytes).unwrap();
        client.read_tls(&mut bytes.as_slice()).unwrap();
        client.process_new_packets()?;
        if !client.is_handshaking() {
            return Ok(());
        }
    }
    panic!("handshake did not complete");
}
