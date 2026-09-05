//! The index client against a real signed index over a real socket.
//!
//! Everything here goes through [`mixengine_testkit::MockRegistry`], which generates its own keypair
//! and signs with `minisign` — the signing half of the crate pair whose verifying half the client
//! links. So these assert that the client accepts what minisign actually produces rather than what
//! we believe it produces, which matters because the format has a legacy variant the client refuses
//! on purpose and a hand-built fixture would have hidden the difference.

use std::path::Path;
use std::time::{Duration, SystemTime};

use mixengine_core::index::{Client, Freshness, Index};
use mixengine_testkit::MockRegistry;

/// An index with an artifact for every platform CI runs on, so `artifact()` answers wherever this
/// test is executed rather than only on the machine it was written on.
fn index_at(generated_at: &str) -> serde_json::Value {
    let artifact = |os: &str, arch: &str, binary: &str| {
        serde_json::json!({
            "os": os, "arch": arch,
            "url": format!("https://example.invalid/php-8.3.33-{os}-{arch}.zip"),
            "sha256": "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff",
            "size": 34_718_139u64,
            "provides": { "php": binary }
        })
    };

    serde_json::json!({
        "schema": 1,
        "generated_at": generated_at,
        "packages": [{
            "kind": "php",
            "version": "8.3.33",
            "channel": "stable",
            "eol": "2027-12-31",
            "artifacts": [
                artifact("windows", "x86_64", "php.exe"),
                artifact("macos", "aarch64", "bin/php"),
                artifact("linux", "x86_64", "bin/php"),
                artifact("linux", "aarch64", "bin/php"),
            ]
        }]
    })
}

/// Move the cached document's mtime backwards.
///
/// The freshness window is six hours and a test cannot wait one, so the clock is not what moves —
/// the file is. That also exercises the real reading: the client takes the age from the document's
/// mtime rather than from a sidecar of its own.
fn age_cache(cache: &Path, by: Duration) {
    let file = std::fs::File::options()
        .write(true)
        .open(cache.join("index.json"))
        .expect("the cache was written");
    file.set_modified(SystemTime::now() - by)
        .expect("set the cache mtime");
}

fn client(registry: &MockRegistry, cache: &Path) -> Client {
    Client::with(&registry.url(), registry.public_key(), cache).expect("build a client")
}

#[tokio::test]
async fn a_signed_index_is_fetched_verified_and_read() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;

    let catalogue = client(&registry, cache.path())
        .catalogue()
        .await
        .expect("the index is readable");

    assert_eq!(catalogue.freshness, Freshness::Fetched);
    let chosen = catalogue
        .index
        .artifact("php", "8.3.33")
        .expect("an artifact for the platform this test runs on");
    assert_eq!(chosen.artifact.size, 34_718_139);
    assert!(chosen.artifact.provides.contains_key("php"));

    // Every platform `test` runs on has its own artifact in the fixture above, so nothing here is
    // reached by emulation — which is the reading on five of the six targets and the one this
    // assertion pins, so that a change to the preference order shows up as a failure here too.
    assert_eq!(chosen.execution, mixengine_proto::Execution::Native);
}

#[tokio::test]
async fn a_document_the_signature_does_not_cover_is_refused() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;

    // The one move a real attacker gets against a client that checks nothing: change the bytes and
    // leave the old signature in place.
    registry.publish_unsigned(&index_at("2026-09-01T00:00:00Z"));

    let refusal = client(&registry, cache.path())
        .catalogue()
        .await
        .expect_err("a document nobody signed is not an index");
    assert!(
        matches!(refusal, mixengine_core::Error::IndexSignature { .. }),
        "expected a signature refusal, got {refusal:?}"
    );
}

#[tokio::test]
async fn an_index_signed_by_somebody_else_is_refused() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let ours = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;
    let theirs = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;

    // Perfectly valid, and signed by the wrong key — a mirror serving somebody else's index.
    let client: Client<Index> =
        Client::with(&theirs.url(), ours.public_key(), cache.path()).expect("a client");

    let refusal = client
        .catalogue()
        .await
        .expect_err("another key is not this build's key");
    assert!(
        matches!(refusal, mixengine_core::Error::IndexSignature { .. }),
        "expected a signature refusal, got {refusal:?}"
    );
}

#[tokio::test]
async fn a_fresh_cache_is_used_without_asking_the_network() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;
    let client = client(&registry, cache.path());

    assert_eq!(
        client.catalogue().await.expect("first fetch").freshness,
        Freshness::Fetched
    );

    // If the second call went to the network it would now fail, so answering at all is the proof.
    registry.unplug();

    let second = client.catalogue().await.expect("served from the cache");
    assert!(
        matches!(second.freshness, Freshness::Cached { .. }),
        "expected a cache hit, got {:?}",
        second.freshness
    );
    assert!(second.index.artifact("php", "8.3.33").is_some());
}

#[tokio::test]
async fn a_stale_cache_is_served_when_the_network_is_gone() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;
    let client = client(&registry, cache.path());

    client.catalogue().await.expect("first fetch");
    age_cache(cache.path(), Duration::from_secs(48 * 60 * 60));
    registry.unplug();

    let stale = client
        .catalogue()
        .await
        .expect("an old index is still an index");
    assert!(
        stale.freshness.is_stale(),
        "expected staleness to be reported, got {:?}",
        stale.freshness
    );
    // The whole point of serving it: a version list from two days ago still installs PHP 8.3.33.
    assert!(stale.index.artifact("php", "8.3.33").is_some());
}

#[tokio::test]
async fn an_index_from_before_the_cached_one_is_refused_and_the_cache_kept() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&index_at("2026-09-01T00:00:00Z")).await;
    let client = client(&registry, cache.path());

    client.catalogue().await.expect("first fetch");

    // Correctly signed, by the right key, and older: a stale CDN edge, or a copy replayed from
    // before a security release. The signature cannot tell it apart from the current one.
    registry.publish(&index_at("2026-08-14T06:55:12Z"));
    age_cache(cache.path(), Duration::from_secs(48 * 60 * 60));

    let kept = client.catalogue().await.expect("the cached index is kept");
    assert!(
        kept.freshness.is_stale(),
        "the refusal has to be visible, got {:?}",
        kept.freshness
    );
    assert_eq!(
        kept.index.generated_at.to_string(),
        "2026-09-01T00:00:00Z",
        "the newer document must survive being offered an older one"
    );
}

#[tokio::test]
async fn a_schema_this_build_cannot_read_is_refused() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let mut future = index_at("2026-08-14T06:55:12Z");
    future["schema"] = serde_json::json!(2);
    let registry = MockRegistry::start(&future).await;

    let refusal = client(&registry, cache.path())
        .catalogue()
        .await
        .expect_err("a newer document version is not readable");
    assert!(
        matches!(
            refusal,
            mixengine_core::Error::IndexSchema {
                found: 2,
                expected: 1,
                ..
            }
        ),
        "expected a schema refusal, got {refusal:?}"
    );
}

/// The compiled-in key against the index that is actually published.
///
/// **`#[ignore]`d, and it is the only test in this workspace that reaches the internet.** The suite
/// runs with egress blocked on purpose, so this cannot be part of it — but the thing it checks is
/// exactly the thing every other test here cannot: that [`mixengine_core::index::PUBLIC_KEY`] and
/// [`mixengine_core::index::DEFAULT_URL`] still describe reality. `MockRegistry` proves the client
/// accepts a correctly signed index; only this proves it accepts *ours*.
///
/// Run it deliberately, after a key rotation or a change to the publishing pipeline:
///
/// ```text
/// cargo test -p mixengine-core --test index -- --ignored --nocapture
/// ```
#[tokio::test]
#[ignore = "reaches the internet; the suite runs with egress blocked"]
async fn the_published_index_verifies_against_the_key_in_this_build() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let catalogue = Client::new(cache.path())
        .expect("the compiled-in key parses")
        .catalogue()
        .await
        .expect("the published index verifies against the compiled-in key");

    let php: Vec<_> = catalogue.index.installable("php").collect();
    assert!(
        !php.is_empty(),
        "the published index offers no PHP this machine can install"
    );
    for package in php {
        println!("php {} ({:?})", package.version, package.channel);
    }
}

#[tokio::test]
async fn a_cache_somebody_rewrote_is_ignored_rather_than_trusted() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&index_at("2026-08-14T06:55:12Z")).await;
    let client = client(&registry, cache.path());

    client.catalogue().await.expect("first fetch");

    // The cache is an ordinary file in the user's home. Anything on this machine can rewrite it,
    // which is why it is verified on the way in rather than trusted because we wrote it once.
    std::fs::write(
        cache.path().join("index.json"),
        serde_json::to_vec(&index_at("2099-01-01T00:00:00Z")).expect("serialise"),
    )
    .expect("rewrite the cache");

    let recovered = client.catalogue().await.expect("the network still answers");
    assert_eq!(
        recovered.freshness,
        Freshness::Fetched,
        "a tampered cache must send the client back to the network"
    );
    assert_eq!(
        recovered.index.generated_at.to_string(),
        "2026-08-14T06:55:12Z"
    );
}
