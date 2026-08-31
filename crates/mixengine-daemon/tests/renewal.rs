//! Renewal on a clock, against a real `mixengined` — roadmap task **T52**.
//!
//! What the unit tests beside `certs::renewal` prove is that a report becomes the right decision.
//! What is proved here is the part only a daemon can be wrong about: that the loop runs on its own
//! timer at all, and that a certificate it replaces is handed to the generator rather than left in
//! a directory nothing re-reads.
//!
//! The daemon handle is held by each test rather than by a fixture, because one of them has to stop
//! it and start another against the same home.

use std::path::Path;
use std::process::{Child, Command, Stdio};

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::Home;
use serde_json::{Value, json};

struct Daemon(Child);

impl Daemon {
    fn start(home: &Home) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_mixengined"))
                .arg("--home")
                .arg(home.path())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the daemon binary runs"),
        )
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Killed rather than asked, on `tests/sites.rs`' reasoning: a test that failed halfway must
        // not leave a process holding the temporary home open.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One connection to the daemon. The same helpers `tests/sites.rs` carries.
struct Client {
    sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
}

impl Client {
    async fn connect(home: &Home) -> Self {
        let connection = Connection::connect(home.endpoint())
            .await
            .expect("the daemon is listening");

        let (sender, driver) = hyper::client::conn::http1::handshake(TokioIo::new(connection))
            .await
            .expect("the daemon speaks HTTP/1.1");

        tokio::spawn(driver);

        Self { sender }
    }

    async fn call(&mut self, method: &str, params: Value) -> Value {
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

        self.sender
            .ready()
            .await
            .expect("the connection is still open");

        let response = self.sender.send_request(request).await.expect("an answer");
        assert_eq!(response.status(), StatusCode::OK, "{method}");

        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a whole body")
            .to_bytes();

        let answer: Value = serde_json::from_slice(&bytes).expect("a JSON-RPC response");
        assert!(answer.get("error").is_none(), "{method}: {answer}");
        answer["result"].clone()
    }
}

/// A directory to register a project against.
fn repository() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("mixengine-project")
        .tempdir()
        .expect("a temporary directory")
}

/// The certificates directory of a home, spelled the way the daemon spells it.
fn certs_of(home: &Home) -> std::path::PathBuf {
    mixengine_core::Paths::new(
        home.path().to_path_buf(),
        &mixengine_core::config::PathOverrides::default(),
    )
    .certs()
    .to_path_buf()
}

/// Leave this site holding a certificate with twenty days on it, and answer with its bytes.
///
/// **The pair is removed before it is written**, which is not tidiness. `leaf::ensure` asks whether
/// what is already there is reusable *as of the `now` it is given*, and a certificate issued today
/// has a hundred and sixty days left as of seventy days ago — so backdating over one writes nothing
/// at all. That is what the guard below caught the first time this test ran.
///
/// **And it retries**, because the loop under test is running the whole time this happens: it can
/// issue a fresh certificate between the removal and the write, which is the state being arranged
/// rather than a failure. In practice it does not, and one attempt is what this costs.
fn backdate(certs: &Path, certificate: &Path, key: &Path) -> Vec<u8> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

    loop {
        let _ = std::fs::remove_file(certificate);
        let _ = std::fs::remove_file(key);

        mixengine_core::certs::leaf::ensure(
            certs,
            &["blog.test".to_owned()],
            None,
            std::time::SystemTime::now() - std::time::Duration::from_secs(70 * 24 * 60 * 60),
        )
        .expect("a backdated certificate is written");

        let written = std::fs::read(certificate).expect("the backdated certificate is readable");
        let state =
            mixengine_core::certs::leaf::read(certs, "blog.test", std::time::SystemTime::now());

        if matches!(
            &state,
            mixengine_proto::CertState::Present { cert }
                if cert.days_left <= mixengine_core::certs::leaf::RENEW_WITHIN_DAYS
        ) {
            return written;
        }

        assert!(
            std::time::Instant::now() < deadline,
            "this site could not be left holding a certificate that is running out: {state:?}"
        );
    }
}

/// A project, and one site under it that declares HTTPS.
async fn a_site_with_https(home: &Home, repository: &Path) {
    let mut client = Client::connect(home).await;

    client
        .call(
            "project.create",
            json!({"root": repository.display().to_string(), "name": "blog"}),
        )
        .await;

    client
        .call(
            "site.create",
            json!({
                "project": {"name": "blog"},
                "domains": ["blog.test"],
                "kind": {"kind": "static"},
                "https": true,
            }),
        )
        .await;
}

/// **A running daemon renews, and the front end is told** — roadmap task **T52**.
///
/// The certificate assertion on its own would pass for a renewal wired only to the certificate
/// directory, which is exactly the failure this task exists to prevent — so the line saying the
/// generator ran is the half that matters, and it is one line rather than two so that it cannot be
/// written by a renewal that never got that far.
#[tokio::test]
async fn a_running_daemon_renews_a_certificate_that_is_running_out() {
    let home = Home::configured("[certs]\nrenew_check_seconds = 1\n");
    let _daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    let repository = repository();
    a_site_with_https(&home, repository.path()).await;

    let certs = certs_of(&home);
    let certificate = mixengine_core::certs::leaf::certificate_path(&certs, "blog.test");
    let key = mixengine_core::certs::leaf::key_path(&certs, "blog.test");
    assert!(certificate.exists(), "the site was issued no certificate");

    let backdated = backdate(&certs, &certificate, &key);

    home.wait_until_daemon_log_says("renewed certificates and told the front end")
        .await;

    let renewed = std::fs::read(&certificate).expect("the renewed certificate is readable");
    assert_ne!(
        renewed, backdated,
        "the daemon said it renewed and the certificate on disk is still the old one"
    );
}

/// **The half of T52 that already worked and had no test behind it.**
///
/// Every start issues for every site that declares HTTPS, which is what keeps a machine switched
/// off more than it is on current forever. Nothing asserted it: every suite that exercises issuance
/// creates its site through the API, which issues on its own path, so a start that stopped issuing
/// would have gone unnoticed until somebody's padlock went red.
///
/// **The daemon is killed rather than asked to stop**, on `tests/lifecycle.rs`' reasoning: what is
/// under test is what a *start* does, and a stop that ran its destructors would be a second thing
/// this test depends on.
#[tokio::test]
async fn a_start_issues_for_a_site_whose_certificate_is_missing() {
    let home = Home::new();
    let daemon = Daemon::start(&home);
    home.wait_until_listening().await;

    let repository = repository();
    a_site_with_https(&home, repository.path()).await;

    let certs = certs_of(&home);
    let certificate = mixengine_core::certs::leaf::certificate_path(&certs, "blog.test");
    let key = mixengine_core::certs::leaf::key_path(&certs, "blog.test");
    assert!(certificate.exists(), "the site was never issued one");

    // The state a home restored from a backup that skipped `certs/sites/` comes back in.
    std::fs::remove_file(&certificate).expect("the certificate is removed");
    std::fs::remove_file(&key).expect("the key is removed");

    drop(daemon);
    home.wait_until_gone().await;

    let _second = Daemon::start(&home);

    // **Waited for by the log and not by the endpoint.** The listener is bound before the start
    // issues anything, so a connection can be answered while this is still to come. The line
    // appears only on this second start: the first issued nothing, because the site did not exist
    // yet when it ran.
    home.wait_until_daemon_log_says("signed certificates for this home's sites")
        .await;

    assert!(
        certificate.exists(),
        "a start left a site declaring HTTPS with no certificate"
    );
}
