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

use std::collections::BTreeMap;
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
    assets: BTreeMap<String, Vec<u8>>,
    cut_after: Option<usize>,
    ranges: Vec<Option<String>>,
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
            assets: BTreeMap::new(),
            cut_after: None,
            ranges: Vec::new(),
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

    /// Serve `bytes` at `path`, and answer with the URL that reaches them.
    ///
    /// This is what turns the registry from an index server into one an install can actually
    /// download from: the artifact a [`FakePackage`](crate::FakePackage) packed goes here, and the
    /// URL goes in the index document served beside it.
    ///
    /// **Range requests are honoured**, which is not decoration — a client that resumes a download
    /// is only resuming if the server is one that can be resumed from, and a mock that ignored the
    /// header would let a client which never sent one pass every test.
    pub fn publish_asset(&self, path: &str, bytes: Vec<u8>) -> String {
        self.published
            .lock()
            .expect("the registry lock")
            .assets
            .insert(path.to_owned(), bytes);

        format!("http://{}{path}", self.address)
    }

    /// End the next asset response after `bytes`, once.
    ///
    /// A connection dropped mid-file, which is the case resuming exists for. The body simply stops:
    /// the client is left holding a prefix, which is exactly what it is expected to notice and
    /// continue from. Consumed by the response it truncates, so the attempt after it succeeds.
    pub fn cut_next_response_after(&self, bytes: usize) {
        self.published.lock().expect("the registry lock").cut_after = Some(bytes);
    }

    /// The `Range` header of every asset request so far, in order, [`None`] where there was none.
    ///
    /// What a test asserts a resume on. "The file arrived eventually" is true of a client that
    /// downloaded it twice from the start.
    #[must_use]
    pub fn asset_ranges(&self) -> Vec<Option<String>> {
        self.published
            .lock()
            .expect("the registry lock")
            .ranges
            .clone()
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

/// The document, the signature beside it, and whatever artifacts were published.
async fn answer(
    published: Arc<Mutex<Published>>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let (body, status) = {
        let mut published = published.lock().expect("the registry lock");
        if !published.reachable {
            (Bytes::new(), StatusCode::SERVICE_UNAVAILABLE)
        } else {
            match request.uri().path() {
                "/index.json" => (Bytes::from(published.document.clone()), StatusCode::OK),
                "/index.json.minisig" => (
                    Bytes::from(published.signature.clone().into_bytes()),
                    StatusCode::OK,
                ),
                path => match published.assets.get(path).cloned() {
                    Some(asset) => asset_answer(&mut published, &request, asset),
                    None => (Bytes::new(), StatusCode::NOT_FOUND),
                },
            }
        }
    };

    Ok(Response::builder()
        .status(status)
        .body(Full::new(body))
        .expect("a response with no invalid headers"))
}

/// Serve an artifact, honouring `Range` and truncating when told to.
///
/// Only the open-ended `bytes=N-` form is understood, because that is the only one a resuming
/// download sends. Anything else is served whole, which is what a server that does not do ranges
/// does — and the client is expected to notice the `200` and start over rather than append.
fn asset_answer(
    published: &mut Published,
    request: &Request<hyper::body::Incoming>,
    asset: Vec<u8>,
) -> (Bytes, StatusCode) {
    let range = request
        .headers()
        .get(hyper::header::RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    published.ranges.push(range.clone());

    let from = range
        .as_deref()
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.strip_suffix('-'))
        .and_then(|value| value.parse::<usize>().ok());

    let (body, status) = match from {
        Some(from) if from >= asset.len() => (Vec::new(), StatusCode::RANGE_NOT_SATISFIABLE),
        Some(from) => (asset[from..].to_vec(), StatusCode::PARTIAL_CONTENT),
        None => (asset, StatusCode::OK),
    };

    // Truncation is applied after the range, so a cut second response is a cut *resume*.
    let body = match published.cut_after.take() {
        Some(cut) if cut < body.len() => body[..cut].to_vec(),
        _ => body,
    };

    (Bytes::from(body), status)
}
