use super::*;
use pretty_assertions::assert_eq;

fn provider() -> Provider {
    Provider {
        name: "test".into(),
        base_url: "https://model.test/v1".into(),
        query_params: None,
        headers: HeaderMap::new(),
        retry: crate::RetryConfig {
            max_attempts: 1,
            base_delay: std::time::Duration::ZERO,
            retry_429: false,
            retry_5xx: false,
            retry_transport: false,
        },
        stream_idle_timeout: std::time::Duration::from_secs(1),
        max_output_tokens: None,
        hosted_results_replay: None,
    }
}

fn headers(value: &str) -> HeaderMap {
    [(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_str(value).unwrap(),
    )]
    .into_iter()
    .collect()
}

#[test]
fn actual_header_and_private_query_changes_allocate_new_identity() {
    let mut cache = InstanceCache::default();
    let mut provider = provider();
    let first = cache
        .identify(&provider, headers("Bearer first-secret"), || {
            Ok("first".into())
        })
        .unwrap();
    assert_eq!(
        cache
            .identify(&provider, headers("Bearer first-secret"), || panic!(
                "cache hit"
            ))
            .unwrap(),
        first
    );
    assert_eq!(
        cache
            .identify(&provider, headers("Bearer second-secret"), || Ok(
                "second".into()
            ))
            .unwrap(),
        Some("second".into())
    );
    provider.query_params = Some([("api-key".into(), "query-secret".into())].into());
    assert_eq!(
        cache
            .identify(&provider, headers("Bearer first-secret"), || Ok(
                "query".into()
            ))
            .unwrap(),
        Some("query".into())
    );
    provider.base_url.push_str("?token=uri-secret");
    assert_eq!(
        cache
            .identify(&provider, headers("Bearer first-secret"), || Ok(
                "uri".into()
            ))
            .unwrap(),
        Some("uri".into())
    );
}

#[test]
fn cache_payload_capacity_eviction_restart_and_entropy_failure_are_bounded() {
    let provider = provider();
    let mut cache = InstanceCache::default();
    let oversized = "s".repeat(MAX_ENTRY_BYTES);
    assert_eq!(
        cache
            .identify(&provider, headers(&oversized), || panic!(
                "oversized must not allocate"
            ))
            .unwrap(),
        None
    );
    for index in 0..=MAX_ENTRIES {
        cache
            .identify(&provider, headers(&format!("Bearer key-{index}")), || {
                Ok(format!("id-{index}"))
            })
            .unwrap();
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert_eq!(
        cache
            .identify(&provider, headers("Bearer key-0"), || Ok("evicted".into()))
            .unwrap(),
        Some("evicted".into())
    );
    assert_eq!(
        InstanceCache::default()
            .identify(&provider, headers("Bearer key-0"), || Ok(
                "new-process".into()
            ))
            .unwrap(),
        Some("new-process".into())
    );
    let failure = cache.identify(&provider, headers("Bearer error"), || {
        Err(AuthError::Build("randomness unavailable".into()))
    });
    assert!(failure.is_err());
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
}

#[test]
fn process_registry_returns_only_random_non_secret_identity() {
    let provider = provider();
    let a = credential_instance_identity(&provider, headers("Bearer hidden-secret"))
        .unwrap()
        .unwrap();
    let b = credential_instance_identity(&provider, headers("Bearer hidden-secret"))
        .unwrap()
        .unwrap();
    assert_eq!(a, b);
    assert!(!a.contains("hidden-secret"));
    assert!(a.starts_with("credential-instance-v1:"));
}
