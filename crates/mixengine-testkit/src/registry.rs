//! A package index served over a real socket, signed with a real key.
//!
//! `.claude/standards/testing.md` forbids network access in tests and CI blocks egress to enforce
//! it, so everything that reads an index has to read one from here. Deliberately **not** a fake
//! `Client`: the parts most worth testing are the signature check and the cache policy, and a
//! double that skipped either would be a test of the double.
//!
//! # It generates its own key, and that is the point
//!
//! A test cannot hold the production private key, so [`MockRegistry`] makes a fresh keypair per
//! instance and hands out its public half for `mixengine_core::index::Client::with` — named in
//! prose rather than linked, because this crate does not depend on `mixengine-core` and must not
//! start. That forces the key to be injectable in the product rather than hard-wired, and an
//! injectable key is what lets `MIXENGINE_INDEX_URL` point at a team mirror at all.
//!
//! The signature is produced by `minisign`, the signing half of the same author's pair of crates
//! that `minisign-verify` is the verifying half of. So a test proves the client accepts what
//! minisign actually produces, rather than what we believe it produces — which is the difference
//! that matters, since the format has a legacy variant the client refuses on purpose.

use std::convert::Infallible;
use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use minisign::{KeyPair, SecretKey};

/// What the server is currently prepared to say.
#[derive(Debug)]
struct Published {
    document: Vec<u8>,
    signature: String,
    reachable: bool,
}

/// An in-process registry: one index, one signature, one switch for pulling the plug.
#[derive(Debug)]
pub struct MockRegistry {
    address: SocketAddr,
    public_key: String,
    secret_key: SecretKey,
    published: Arc<Mutex<Published>>,
}

impl MockRegistry {
    /// Start a registry serving `index`, and return once it is accepting connections.
    ///
    /// Bound to port 0 on loopback, so any number of these run at once without a port to allocate
    /// or a fixture to serialise on.
    ///
    /// # Panics
    ///
    /// If a keypair cannot be generated, the document cannot be signed, or the socket cannot be
    /// bound — each of which means the test environment is broken rather than the code under test.
    #[must_use]
    pub async fn start(index: &serde_json::Value) -> Self {
        let pair = KeyPair::generate_unencrypted_keypair().expect("generate a minisign keypair");
        let public_key = pair.pk.to_base64();

        let document = serde_json::to_vec_pretty(index).expect("serialise the index");
        let signature = sign(&pair.sk, &document);

        let published = Arc::new(Mutex::new(Published {
            document,
            signature,
            reachable: true,
        }));

        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind a loopback port");
        let address = listener.local_addr().expect("read the bound port");

        let serving = Arc::clone(&published);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = Arc::clone(&serving);
                tokio::spawn(async move {
                    let service = service_fn(move |request| answer(Arc::clone(&state), request));
                    // Errors here are a client that hung up mid-request, which several of these
                    // tests do on purpose.
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });

        Self {
            address,
            public_key,
            secret_key: pair.sk,
            published,
        }
    }

    /// The URL of the index document, which is what a client is pointed at.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}/index.json", self.address)
    }

    /// The base64 public key this registry signs with.
    #[must_use]
    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Replace what is served, re-signed with the same key.
    ///
    /// # Panics
    ///
    /// If the document cannot be serialised or signed.
    pub fn publish(&self, index: &serde_json::Value) {
        let document = serde_json::to_vec_pretty(index).expect("serialise the index");
        let signature = sign(&self.secret_key, &document);
        let mut published = self.published.lock().expect("the registry lock");
        published.document = document;
        published.signature = signature;
    }

    /// Serve a document with a signature that does not cover it.
    ///
    /// The one tampering a real attacker gets to attempt against a client that checks nothing: the
    /// bytes are changed and the old signature is left in place.
    ///
    /// # Panics
    ///
    /// If the document cannot be serialised.
    pub fn publish_unsigned(&self, index: &serde_json::Value) {
        let document = serde_json::to_vec_pretty(index).expect("serialise the index");
        let mut published = self.published.lock().expect("the registry lock");
        published.document = document;
    }

    /// Stop answering, as a machine with no network does.
    ///
    /// Answers `503` rather than dropping the connection, so a test that exercises the offline path
    /// finishes immediately instead of waiting out a connect timeout. What the client sees either
    /// way is "the index could not be fetched", which is the only distinction it makes.
    pub fn unplug(&self) {
        self.published.lock().expect("the registry lock").reachable = false;
    }

    /// Answer again.
    pub fn plug(&self) {
        self.published.lock().expect("the registry lock").reachable = true;
    }
}

/// Sign `document` the way the publishing pipeline does.
fn sign(secret_key: &SecretKey, document: &[u8]) -> String {
    minisign::sign(
        None,
        secret_key,
        Cursor::new(document),
        Some("timestamp:0\tfile:index.json\thashed"),
        None,
    )
    .expect("sign the index")
    .into_string()
}

/// Two paths and nothing else: the document, and the signature beside it.
async fn answer(
    published: Arc<Mutex<Published>>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (body, status) = {
        let published = published.lock().expect("the registry lock");
        if !published.reachable {
            (Bytes::new(), StatusCode::SERVICE_UNAVAILABLE)
        } else {
            match request.uri().path() {
                "/index.json" => (Bytes::from(published.document.clone()), StatusCode::OK),
                "/index.json.minisig" => (
                    Bytes::from(published.signature.clone().into_bytes()),
                    StatusCode::OK,
                ),
                _ => (Bytes::new(), StatusCode::NOT_FOUND),
            }
        }
    };

    Ok(Response::builder()
        .status(status)
        .body(Full::new(body))
        .expect("a response with no invalid headers"))
}
