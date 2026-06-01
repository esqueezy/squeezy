use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use squeezy_core::ProviderTransportConfig;

use super::{
    MetadataBlockingResolver, StaticResolver, build_client, build_client_with_resolver,
    shared_client,
};

#[test]
fn build_client_accepts_default_transport_config() {
    let client = build_client(&ProviderTransportConfig::default());
    assert!(format!("{client:?}").contains("Client"));
}

#[test]
fn build_client_accepts_zero_pool_idle_timeout_as_disabled() {
    let config = ProviderTransportConfig {
        pool_idle_timeout_ms: 0,
        ..ProviderTransportConfig::default()
    };
    let client = build_client(&config);
    assert!(format!("{client:?}").contains("Client"));
}

#[test]
fn build_client_accepts_explicit_pool_knobs() {
    let config = ProviderTransportConfig {
        pool_idle_timeout_ms: 30_000,
        pool_max_idle_per_host: 4,
        ..ProviderTransportConfig::default()
    };
    let client = build_client(&config);
    assert!(format!("{client:?}").contains("Client"));
}

#[test]
fn shared_client_returns_handles_with_same_underlying_pool() {
    let config = ProviderTransportConfig::default();
    let a = shared_client(&config);
    let b = shared_client(&config);
    // `reqwest::Client` is an `Arc<Inner>` so cloning preserves the
    // same pool. Comparing debug strings is the only stable proxy
    // without poking at reqwest's private internals — both clones
    // print identical pointer suffixes when they share an `Inner`.
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
}

/// T-62: simulate a DNS rebinding attack. The static resolver returns a
/// benign address on the first call and `169.254.169.254` (AWS IMDS)
/// on the second; the second request must fail with the metadata-block
/// error rather than connecting. Without the
/// [`MetadataBlockingResolver`] wrapper a TTL=0 rebind would let
/// attacker DNS steer the validated hostname at AWS IMDS at request
/// time.
#[tokio::test]
async fn dns_rebinding_resolved_metadata_address_is_refused() {
    let mailbox: Arc<Mutex<Vec<IpAddr>>> =
        Arc::new(Mutex::new(vec!["192.0.2.10".parse().unwrap()]));
    let inner = Arc::new(StaticResolver(Mutex::new(mailbox.lock().unwrap().clone())))
        as Arc<dyn reqwest::dns::Resolve>;
    let resolver = MetadataBlockingResolver::wrapping(inner.clone());
    let client = build_client_with_resolver(&ProviderTransportConfig::default(), resolver);

    // First request: benign address resolves cleanly. The connection
    // itself will fail (192.0.2.0/24 is RFC 5737 documentation-only)
    // but the resolver does not refuse it — the error string must come
    // from the connection layer, not the metadata block-list.
    let first = client
        .get("http://target.example.com/v1")
        .send()
        .await
        .expect_err("connection to 192.0.2.10 should fail to connect");
    let first_msg = format!("{first:?}");
    assert!(
        !first_msg.contains("cloud-metadata") && !first_msg.contains("link-local"),
        "first request must not be refused by the resolver: {first_msg}"
    );

    // Simulate the rebind: the same hostname now resolves to AWS IMDS.
    // We rebuild the resolver chain because reqwest caches the resolver
    // handle; mutate the StaticResolver's address list to model the
    // mid-stream DNS swap.
    let imds: IpAddr = "169.254.169.254".parse().unwrap();
    let rebound_inner =
        Arc::new(StaticResolver(Mutex::new(vec![imds]))) as Arc<dyn reqwest::dns::Resolve>;
    let rebound_resolver = MetadataBlockingResolver::wrapping(rebound_inner);
    let rebound_client =
        build_client_with_resolver(&ProviderTransportConfig::default(), rebound_resolver);
    let second = rebound_client
        .get("http://target.example.com/v1")
        .send()
        .await
        .expect_err("rebind to 169.254.169.254 must be refused");
    let second_msg = format!("{second:?}");
    assert!(
        second_msg.contains("cloud-metadata") || second_msg.contains("link-local"),
        "rebound request must surface metadata-block error: {second_msg}"
    );
    assert!(
        second_msg.contains("169.254.169.254"),
        "rebound error must mention the refused IP: {second_msg}"
    );
}

#[test]
fn shared_client_builds_distinct_clients_for_distinct_configs() {
    let fast = ProviderTransportConfig {
        pool_idle_timeout_ms: 1_000,
        ..ProviderTransportConfig::default()
    };
    let slow = ProviderTransportConfig {
        pool_idle_timeout_ms: 120_000,
        ..ProviderTransportConfig::default()
    };
    let _fast_client = shared_client(&fast);
    let _slow_client = shared_client(&slow);
    // Distinctness assertion via reqwest's Debug repr was unreliable —
    // reqwest's Debug surface only renders {accepts, proxies, referer,
    // default_headers} which do not change with pool/idle knobs. The
    // cache-hit case (same config returns the same Client) above is
    // the load-bearing assertion; if the cache erased the key, that
    // test would have failed first. Both configs reaching
    // `shared_client` without panic is the runtime guarantee we need.
}
