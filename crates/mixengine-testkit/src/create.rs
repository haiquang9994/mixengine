//! Declaring a fixture service the way a person does: over the socket, through `service.create`.
//!
//! **What replaced half of [`crate::declare`]** in roadmap task **T31a**. Until the daemon had a
//! method for it, every suite that drove a service wrote its `services` row by hand — which meant
//! the row every supervision test ran against was one no shipped code path had ever produced. Now
//! the only row written by hand is the `packages` one ([`declare::package`](crate::declare::package)),
//! because `fakeservice` is a fixture binary no index will ever publish; the service itself arrives
//! the way a user's does, and a difference between the two would fail a suite rather than hide.
//!
//! A small JSON-RPC client rather than driving `mix`, because what a fixture has to send is the
//! `overrides` document — how the fake service is to behave — and the command line deliberately has
//! no flag for handing a service arbitrary settings.

use std::path::Path;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::{Connection, Endpoint};
use serde_json::{Value, json};

use crate::declare::{self, Service, VERSION};

/// Declare `services` in the home a daemon is serving at `endpoint`.
///
/// Writes the fixture's `packages` row first, then creates each service. The daemon has to be
/// listening: the schema is created when one opens the home, and `service.create` is what this is
/// for.
///
/// Ids must be `fakeservice` or `fakeservice@<name>` — the part before the `@` is the package a
/// service is an instance of, so an id that says anything else is asking for a package no fixture
/// installed.
///
/// # Panics
///
/// If the daemon cannot be reached, or if any create is refused — a fixture that half worked would
/// fail later as an assertion about the daemon, which is the wrong thing to go looking at.
pub async fn create(endpoint: &Endpoint, database: &Path, services: &[Service]) {
    declare::package(database).await;

    for service in services {
        let answer = call(
            endpoint,
            "service.create",
            json!({
                "id": service.id(),
                "version": VERSION,
                "overrides": service.overrides(),
            }),
        )
        .await;

        assert!(
            answer.get("error").is_none(),
            "declaring `{}`: {answer}",
            service.id()
        );
    }
}

/// [`create`], for a test that has no runtime of its own.
///
/// The end-to-end suites drive `mix` through [`std::process::Command`] and are plain `#[test]`
/// functions; building a runtime for a handful of calls is cheaper than making every one of them
/// `async`.
///
/// # Panics
///
/// As [`create`], and if a runtime cannot be started.
pub fn create_blocking(endpoint: &Endpoint, database: &Path, services: &[Service]) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a current-thread runtime")
        .block_on(create(endpoint, database, services));
}

/// One JSON-RPC call, and the whole response object.
async fn call(endpoint: &Endpoint, method: &str, params: Value) -> Value {
    let connection = Connection::connect(endpoint)
        .await
        .unwrap_or_else(|error| panic!("the daemon is listening on {endpoint}: {error}"));

    let (mut sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(connection))
        .await
        .expect("the daemon speaks HTTP/1.1");

    tokio::spawn(driver);

    let body = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/rpc")
        .header(HOST, "mixengine")
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("a request serialises"),
        )))
        .expect("a well formed request");

    let response = sender
        .send_request(request)
        .await
        .unwrap_or_else(|error| panic!("{method}: {error}"));

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a whole body")
        .to_bytes();

    serde_json::from_slice(&bytes).expect("a JSON-RPC response")
}
