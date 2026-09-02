//! The extension registry against a real signed document over a real socket — roadmap task **T81**.
//!
//! `tests/index.rs` one document along, and deliberately the same shape: the client is generic over
//! what it reads (the T81 design's D3), so what these prove is that the *second* document gets the
//! treatment the first one gets — verified before it is parsed, cached, and refused when it walks
//! backwards — rather than a second implementation that resembles it.
//!
//! The one thing here that has no counterpart there is the entry the listing skips: a document can
//! be entirely ours, entirely valid, and still hold an extension a build this old cannot read.

use std::path::Path;

use mixengine_core::extensions::registry::Registry;
use mixengine_core::index::{Client, Freshness};
use mixengine_testkit::MockRegistry;

/// A registry document holding `entries`, generated at `generated_at`.
fn registry_at(generated_at: &str, entries: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": generated_at,
        "extensions": entries,
    })
}

/// One published entry: a fixture manifest, in the shape the document carries it.
fn entry(text: &str) -> serde_json::Value {
    let manifest = mixengine_core::extensions::manifest::read(Path::new("extension.toml"), text)
        .expect("a fixture parses");

    mixengine_core::extensions::manifest::to_value(&manifest)
}

fn client(registry: &MockRegistry, cache: &Path) -> Client<Registry> {
    Client::with(&registry.url(), registry.public_key(), cache).expect("build a client")
}

/// The ordinary path: a document this build's key vouches for is read, and what it lists is what
/// was published.
#[tokio::test]
async fn a_signed_registry_is_read() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&registry_at(
        "2026-09-02T09:00:00Z",
        vec![entry(mixengine_testkit::extension::MAILPIT)],
    ))
    .await;

    let catalogue = client(&registry, cache.path())
        .catalogue()
        .await
        .expect("a verified registry");

    assert_eq!(catalogue.freshness, Freshness::Fetched);
    let listing = catalogue.index.listing();
    assert_eq!(listing.unreadable, 0);
    assert_eq!(listing.extensions.len(), 1);
    assert_eq!(listing.extensions[0].extension.id.as_str(), "mailpit");
}

/// **An entry this build cannot read costs that entry and nothing else** — the design's D4.
///
/// The document is entirely valid and entirely ours; one of its entries was written by a newer
/// build. Everything else in it still lists, and the count is what tells a person why their
/// extension is not there.
#[tokio::test]
async fn one_entry_from_a_newer_build_does_not_cost_the_registry() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&registry_at(
        "2026-09-02T09:00:00Z",
        vec![
            entry(mixengine_testkit::extension::MAILPIT),
            serde_json::json!({
                "schema": 99,
                "extension": { "id": "from-the-future", "kind": "quantum-tunnel" }
            }),
        ],
    ))
    .await;

    let catalogue = client(&registry, cache.path())
        .catalogue()
        .await
        .expect("a verified registry");

    let listing = catalogue.index.listing();
    assert_eq!(listing.extensions.len(), 1);
    assert_eq!(listing.unreadable, 1);
}

/// A registry signed by another key is refused, exactly as an index is.
#[tokio::test]
async fn a_registry_signed_by_somebody_else_is_refused() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let ours = MockRegistry::start(&registry_at("2026-09-02T09:00:00Z", Vec::new())).await;
    let theirs = MockRegistry::start(&registry_at("2026-09-02T09:00:00Z", Vec::new())).await;

    let client: Client<Registry> =
        Client::with(&theirs.url(), ours.public_key(), cache.path()).expect("a client");

    let refusal = client
        .catalogue()
        .await
        .expect_err("another key is not this build's key");

    let said = refusal.to_string();
    assert!(
        said.contains("is not signed by this build's key"),
        "a document signed by another key was accepted: {said}"
    );
    // **And it says which document.** The error family is shared with the package index (D3), so
    // the one thing that has to be right here is the word that sends somebody to the right place.
    assert!(
        said.contains("extension registry"),
        "a registry failure was reported as the package index: {said}"
    );
}

/// **A correctly signed registry can still be the wrong one.**
///
/// Every version ever published is signed just as validly as the newest, so the signature cannot
/// tell a replayed older document from the current one and `generated_at` is what can. The cached
/// document is kept and the refusal is said out loud, which is the client's answer for every way of
/// failing to get a new one.
#[tokio::test]
async fn a_registry_from_before_the_cached_one_is_refused() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&registry_at(
        "2026-09-02T09:00:00Z",
        vec![entry(mixengine_testkit::extension::MAILPIT)],
    ))
    .await;
    let client = client(&registry, cache.path());

    client.catalogue().await.expect("the first fetch");

    registry.publish(&registry_at("2026-08-01T09:00:00Z", Vec::new()));

    let catalogue = client.catalogue().await.expect("the cache answers");

    assert_eq!(
        catalogue.index.listing().extensions.len(),
        1,
        "the rolled-back document replaced the one already held"
    );
}

/// The two documents do not share a cache file, or a registry fetched now would look like an index
/// rolled back to now.
#[tokio::test]
async fn the_registry_caches_beside_the_index_and_not_over_it() {
    let cache = tempfile::tempdir().expect("a cache directory");
    let registry = MockRegistry::start(&registry_at("2026-09-02T09:00:00Z", Vec::new())).await;

    client(&registry, cache.path())
        .catalogue()
        .await
        .expect("a verified registry");

    assert!(cache.path().join("extensions.json").exists());
    assert!(cache.path().join("extensions.json.minisig").exists());
    assert!(
        !cache.path().join("index.json").exists(),
        "the registry was cached as the package index"
    );
}
