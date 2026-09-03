//! A `web-app` extension's site, against a real `mixengined` — roadmap task **T81b**.
//!
//! `core`'s tests prove the rows; what only a daemon can be wrong about is the surface: that the
//! site an install wrote is listed with its owner, that `site.*` refuses to edit it and allows
//! stopping it, that removing the PHP it runs on is refused by name, and that an uninstall takes
//! it away and says so. The PHP is a row and a pool row (`declare::php_pool`), not a download.

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use mixengine_platform::ipc::Connection;
use mixengine_testkit::{Home, declare};
use serde_json::{Value, json};

/// How long an install of a directory copy may take.
const PATIENCE: Duration = Duration::from_secs(30);

struct Fixture {
    home: Home,
    _daemon: Daemon,
}

impl Fixture {
    async fn start() -> Self {
        let home = Home::new();
        let daemon = Daemon::start(&home);
        home.wait_until_listening().await;

        Self {
            home,
            _daemon: daemon,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.home).await
    }
}

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
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One connection to the daemon. The same helpers `tests/runtimes.rs` carries.
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
        let answer = self.ask(method, params).await;
        assert!(answer.get("error").is_none(), "{method}: {answer}");
        answer["result"].clone()
    }

    async fn refuse(&mut self, method: &str, params: Value) -> Value {
        let answer = self.ask(method, params).await;
        assert!(answer.get("result").is_none(), "{method}: {answer}");
        answer["error"].clone()
    }

    async fn ask(&mut self, method: &str, params: Value) -> Value {
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

        serde_json::from_slice(&bytes).expect("a JSON-RPC response")
    }

    /// Wait for a job to end, and answer with it as it ended.
    async fn finished(&mut self, job: Value) -> Value {
        let deadline = tokio::time::Instant::now() + PATIENCE;

        loop {
            let waited = self
                .call("job.wait", json!({"job": job, "timeout": 2_000}))
                .await;

            if waited["state"] != "running" {
                return waited;
            }

            assert!(
                tokio::time::Instant::now() < deadline,
                "the install never finished: {waited}"
            );
        }
    }
}

/// A directory holding the phpMyAdmin fixture and the doc root it names.
fn web_app() -> tempfile::TempDir {
    let directory = tempfile::Builder::new()
        .prefix("mixengine-web-app")
        .tempdir()
        .expect("a temporary directory");
    std::fs::write(
        directory.path().join("extension.toml"),
        mixengine_testkit::extension::PHPMYADMIN,
    )
    .expect("a manifest");
    // **The archive's own top level** — roadmap task **T82**. `--path` copies what a download would
    // have unpacked, so the directory here is the one `[web-app].root` names.
    std::fs::create_dir_all(directory.path().join("phpMyAdmin-5.2.3-all-languages"))
        .expect("a doc root");

    directory
}

/// Plan, consent, install, wait: the extension is installed and the job succeeded. Answers with
/// the plan, which is what the person read.
async fn installed(client: &mut Client, path: &str, log: &Home) -> Value {
    let source = json!({"type": "path", "path": path});
    let plan = client
        .call("extension.plan", json!({"source": source}))
        .await;
    let started = client
        .call(
            "extension.install",
            json!({
                "source": source,
                "consent": {
                    "id": plan["id"],
                    "version": plan["version"],
                    "signed": plan["signed"],
                    "network": plan["permissions"]["network"],
                },
            }),
        )
        .await;

    let finished = client.finished(started["id"].clone()).await;
    assert_eq!(
        finished["state"],
        "succeeded",
        "{finished}\n{}",
        log.daemon_log()
    );

    plan
}

/// **D4, D5, D6, D8 in one walk.**
#[tokio::test(flavor = "multi_thread")]
async fn a_web_app_is_served_on_a_site_only_its_extension_may_edit() {
    let fixture = Fixture::start().await;
    declare::php_pool(&fixture.home.database_file(), "8.3.34").await;
    // **T82.** The fixture web-app declares a database, so this home has to run one — a plan
    // refused before anything is fetched is the point of that declaration, not an accident here.
    declare::database(
        &fixture.home.database_file(),
        "mariadb@main",
        "mariadb",
        3306,
    )
    .await;
    let mut client = fixture.client().await;
    let directory = web_app();
    let path = directory.path().display().to_string();

    let plan = installed(&mut client, &path, &fixture.home).await;
    assert_eq!(
        plan["site"]["domain"], "phpmyadmin.mixengine.test",
        "{plan}"
    );
    assert_eq!(plan["site"]["pool"], "php-fpm@8.3.34", "{plan}");

    // Listed with its owner, HTTPS on, enabled.
    let listed = client.call("site.list", json!({})).await;
    let sites = listed["sites"].as_array().expect("a list");
    assert_eq!(sites.len(), 1, "{listed}");
    assert_eq!(sites[0]["domain"], "phpmyadmin.mixengine.test");
    assert_eq!(
        sites[0]["owner"],
        json!({"type": "extension", "id": "phpmyadmin"})
    );
    assert_eq!(sites[0]["https"], true);
    assert_eq!(sites[0]["state"], "enabled");

    let extensions = client.call("extension.list", json!({})).await;
    assert_eq!(
        extensions["extensions"][0]["site"], "phpmyadmin.mixengine.test",
        "{extensions}"
    );

    // Its root is the install directory, and its pool is the one the plan named.
    let site = json!({"site": {"domain": "phpmyadmin.mixengine.test"}});
    let shown = client.call("site.show", site.clone()).await;
    assert_eq!(shown["root"], plan["install_dir"], "{shown}");
    // **The archive's own top level** — roadmap task **T82**. `doc_root_exists` is what would
    // report a `[web-app].root` naming a directory the artifact does not unpack to, so it is
    // asserted here beside the path rather than left to a run somebody has to do by hand.
    assert!(
        shown["doc_root_full"]
            .as_str()
            .is_some_and(|full| full.ends_with("phpMyAdmin-5.2.3-all-languages")),
        "{shown}"
    );
    assert_eq!(shown["doc_root_exists"], true, "{shown}");
    assert_eq!(shown["pool"]["declared"], "php-fpm@8.3.34", "{shown}");

    // Every edit is refused with the one sentence; start and stop are not.
    for (method, params) in [
        (
            "site.update",
            json!({"site": {"domain": "phpmyadmin.mixengine.test"}, "https": false}),
        ),
        ("site.delete", site.clone()),
        (
            "site.share",
            json!({"site": {"domain": "phpmyadmin.mixengine.test"}}),
        ),
        (
            "domain.add",
            json!({"site": {"domain": "phpmyadmin.mixengine.test"}, "domain": "pma.test"}),
        ),
    ] {
        let refused = client.refuse(method, params).await;
        assert_eq!(
            refused["data"]["code"], "precondition_failed",
            "{method}: {refused}"
        );
        assert!(
            refused["message"]
                .as_str()
                .is_some_and(|said| said.contains("belongs to the phpmyadmin extension")),
            "{method}: {refused}"
        );
        assert!(
            refused["data"]["hint"]
                .as_str()
                .is_some_and(|hint| hint.contains("mix extension uninstall phpmyadmin")),
            "{method}: {refused}"
        );
    }

    let stopped = client.call("site.stop", site.clone()).await;
    assert_eq!(stopped["site"]["state"], "disabled", "{stopped}");
    let started = client.call("site.start", site.clone()).await;
    assert_eq!(started["site"]["state"], "enabled", "{started}");

    // `extension.start` says what controls it instead.
    let refused = client
        .refuse("extension.start", json!({"id": "phpmyadmin"}))
        .await;
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("mix site stop phpmyadmin.mixengine.test")),
        "{refused}"
    );

    // Uninstall releases the name, and the site is gone.
    let removed = client
        .call("extension.uninstall", json!({"id": "phpmyadmin"}))
        .await;
    assert_eq!(removed["site"], "phpmyadmin.mixengine.test", "{removed}");
    let listed = client.call("site.list", json!({})).await;
    assert!(
        listed["sites"].as_array().is_some_and(Vec::is_empty),
        "{listed}"
    );
}

/// **D9.** The PHP a web-app is frozen on is refused by name, without `--force`.
#[tokio::test(flavor = "multi_thread")]
async fn runtime_uninstall_refuses_for_the_extension_frozen_on_it() {
    let fixture = Fixture::start().await;
    declare::php_pool(&fixture.home.database_file(), "8.3.34").await;
    // **T82.** The fixture web-app declares a database, so this home has to run one — a plan
    // refused before anything is fetched is the point of that declaration, not an accident here.
    declare::database(
        &fixture.home.database_file(),
        "mariadb@main",
        "mariadb",
        3306,
    )
    .await;
    let mut client = fixture.client().await;
    let directory = web_app();
    installed(
        &mut client,
        &directory.path().display().to_string(),
        &fixture.home,
    )
    .await;

    let refused = client
        .refuse(
            "runtime.uninstall",
            json!({"kind": "php", "version": "8.3.34", "force": false}),
        )
        .await;

    assert_eq!(refused["data"]["code"], "precondition_failed", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|said| said.contains("phpmyadmin (extension)")),
        "{refused}"
    );
}

/// **D5.** No PHP inside `requires`: refused at plan, naming what to install.
#[tokio::test(flavor = "multi_thread")]
async fn a_web_app_with_no_matching_php_is_refused_at_plan() {
    let fixture = Fixture::start().await;
    let mut client = fixture.client().await;
    let directory = web_app();

    let refused = client
        .refuse(
            "extension.plan",
            json!({"source": {"type": "path", "path": directory.path().display().to_string()}}),
        )
        .await;

    assert_eq!(refused["data"]["code"], "dependency_missing", "{refused}");
    assert!(
        refused["data"]["hint"]
            .as_str()
            .is_some_and(|hint| hint.contains("mix runtime")),
        "{refused}"
    );
}
