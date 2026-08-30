//! The php-fpm recipe against a **real** PHP — roadmap task **T32**.
//!
//! Everything else about this recipe is provable in one process and is proved there: the template
//! renders, the settings merge, the spec builds, the file and the readiness check name one socket.
//! None of that says the thing the task is about, which is that *a pool MixEngine configured serves
//! a PHP script*. That claim can only be made against the program, so this suite is made against the
//! program — and it is made through the FastCGI protocol, because a pool that is listening and
//! cannot execute anything accepts a connection exactly like one that works.
//!
//! **It is `#[ignore]`d rather than skipped**, for `caddy.rs`' reason: a test that quietly returns
//! when it finds no PHP is a green suite that proved nothing on the day the download broke.
//!
//! **The two systems diverge here on purpose, and the divergence is the assertion.** On Unix the
//! pool is php-fpm on a socket and a changed override is handed to it by `SIGUSR2` — so the same pid
//! serves the new configuration. On Windows it is `php-cgi.exe` on a port with no signal to send, so
//! the running process keeps its old configuration and the suite asserts *that* rather than
//! pretending the two are the same.

mod harness;

#[cfg(windows)]
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use harness::{Home, json};
use mixengine_testkit::fastcgi::Pool;
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked PHP is, as the CI step and a developer both set it.
///
/// The directory the archive unpacks to — `bin/php` inside it on Unix, `php.exe` at its root on
/// Windows — which is also what a `runtime_installs` row's `install_path` is.
const RUNTIME: &str = "MIXENGINE_PHP_RUNTIME";

/// The version the index publishes it as, and the half after the `@` in the pool's id.
const VERSION: &str = "8.3.33";

/// How long the pool is given to be serving again after its configuration moved under it.
///
/// Long for what it covers, because what it is really waiting for is a runner's next turn plus a
/// graceful pool restart on a runner that may be compiling something else at the same time.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// The service this suite drives, which nobody in it creates.
fn pool() -> String {
    format!("php-fpm@{VERSION}")
}

/// The PHP this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(RUNTIME).unwrap_or_else(|| {
        panic!(
            "{RUNTIME} is not set, so there is no PHP to judge this recipe against. The `php` step \
             in .github/workflows/ci.yml fetches one; by hand, unpack any PHP 8.3 from \
             mixengine-packages' releases and point {RUNTIME} at the directory it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and here it is paid for a second reason: the pool's port is
/// allocated by the *install*, so this suite cannot choose it up front the way `caddy.rs` chooses
/// Caddy's — it rebinds afterwards instead.
#[cfg(windows)]
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes, as an index entry says it.
///
/// **Probed rather than written down**, because the layout is the publisher's: `mixengine-packages`
/// puts the Unix binaries under `bin/` and `sbin/` and the Windows ones at the root, and a suite
/// that hard-coded either would pass on one system while describing the other wrongly.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for (name, candidates) in [
        ("php", ["bin/php", "php"].as_slice()),
        ("php-fpm", ["sbin/php-fpm", "bin/php-fpm"].as_slice()),
        ("php-cgi", ["php-cgi", "bin/php-cgi"].as_slice()),
    ] {
        for candidate in candidates {
            let relative = format!("{candidate}{}", std::env::consts::EXE_SUFFIX);

            if root.join(&relative).is_file() {
                found.insert(name.to_owned(), Value::String(relative));
                break;
            }
        }
    }

    // The one that decides whether this suite can run at all. Named rather than left to the recipe,
    // because `ServiceProvidesNothing` arriving from three layers down at `service start` says the
    // same thing much later and much less clearly.
    let sapi = if cfg!(windows) { "php-cgi" } else { "php-fpm" };
    assert!(
        found.contains_key(sapi),
        "{} publishes no {sapi}, so there is nothing here for a pool to run — this suite needs the \
         PHP mixengine-packages builds, not a system one",
        root.display()
    );

    found
}

/// An index offering exactly this PHP, for this machine.
///
/// `"kind": "php"` and not a package name: this is a **runtime**, which is the whole difference T32
/// turns on.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [{
            "kind": "php",
            "version": VERSION,
            "channel": "stable",
            "artifacts": [{
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "url": url,
                "sha256": packed.sha256,
                "size": packed.size(),
                "provides": Value::Object(provides),
            }],
        }],
    })
}

/// What `mix service status <pool>` says.
fn status(home: &Home) -> Value {
    json(&home.mix(&["service", "status", &pool(), "--json"]))
}

/// A home with a real PHP installed in it, a daemon over it, and the endpoint its pool listens on.
///
/// The archive is packed here out of the directory the CI step unpacked, served by a registry that
/// signs its own index, and installed through `runtime.install` — so this suite covers the whole
/// runtime install path against a real artifact on all three systems at no extra cost, and the pool
/// it then drives is the one the post-install hook created rather than one a fixture inserted.
async fn installed() -> (Home, harness::Daemon, MockRegistry, Pool) {
    let root = package();

    let packing = if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    };
    let packed = FakePackage::new(packing)
        .directory(&root)
        .build(&format!("php-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-19T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed = json(&home.mix(&["runtime", "install", "php", VERSION, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

    // Where the pool listens. On Windows it is *rebound* first: the port was allocated from 9000 by
    // the install, this suite could not choose it, and a developer running two of these at once — or
    // one with a php-fpm of their own — would otherwise be fighting over a fixed number. Rebinding
    // is also the one place `services.port` is proved to be what the recipe actually reads.
    #[cfg(windows)]
    let listen = {
        let port = free_port();
        mixengine_testkit::declare::rebind(&home.database_file(), &pool(), port).await;

        Pool::port(std::net::SocketAddr::from(([127, 0, 0, 1], port)))
    };

    #[cfg(unix)]
    let listen = Pool::socket(
        home.path()
            .join("run")
            .join(format!("php-fpm-{VERSION}.sock")),
    );

    (home, daemon, registry, listen)
}

/// **The whole of T32, in the order a user meets it.**
///
/// One test rather than six, deliberately: each step is the previous one's precondition, and six
/// tests would be six real PHP installs performed to re-reach the state this one is already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn a_pool_is_created_started_serves_php_reloaded_and_stopped() {
    let (home, _daemon, _registry, listen) = installed().await;
    let pool = pool();

    // --- created by the install, and by nobody else ---------------------------------------------
    //
    // Nothing in this test asked for a service. The post-install hook did, which is the half of T32
    // that is invisible from inside the daemon and obvious from out here.
    let listed = json(&home.mix(&["service", "list", "--json"]));
    assert!(
        listed["services"]
            .as_array()
            .is_some_and(|services| services.iter().any(|service| service["id"] == pool)),
        "installing a PHP did not give it a pool\n{listed}\n{}",
        home.daemon_log()
    );

    // --- started, and proved up by its own endpoint ----------------------------------------------
    let started = json(&home.mix(&["service", "start", &pool, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let up = status(&home);
    assert_eq!(up["state"], "running", "{up}\n{}", home.daemon_log());
    let pid = up["pid"].as_u64().expect("a running pool has a pid");

    // --- serving PHP ------------------------------------------------------------------------------
    //
    // **The assertion this whole suite exists for.** A pool that is listening and cannot execute
    // anything accepts a connection exactly like one that works, so the claim is made by sending a
    // real FastCGI request and reading back a body only PHP could have produced.
    let script = home.path().join("www").join("hello.php");
    std::fs::create_dir_all(script.parent().expect("a parent")).expect("a document root");
    std::fs::write(
        &script,
        b"<?php echo 'mixengine serves php ', PHP_VERSION, \"\\n\";",
    )
    .expect("a script to serve");

    let answered = listen
        .get(&script)
        .expect("the pool answered a FastCGI request");
    assert!(
        answered.body.contains("mixengine serves php"),
        "the pool is listening and is not running PHP\n{answered:?}\n{}",
        home.daemon_log()
    );
    assert!(
        answered.body.contains(VERSION),
        "the pool is serving a PHP that is not the one installed: {answered:?}"
    );

    // --- handed a configuration that moved under it -----------------------------------------------
    mixengine_testkit::declare::reconfigure(&home.database_file(), &pool, r#"{"max_children": 3}"#)
        .await;

    // Nothing but a listing: the configuration is rendered at the top of every `service.*` call, and
    // a rendering that moved under a running service is handed to it. Nothing here restarts
    // anything.
    let relisted = json(&home.mix(&["service", "list", "--json"]));
    assert!(relisted["services"].is_array(), "{relisted}");

    let deadline = Instant::now() + EVENTUALLY;
    loop {
        if listen
            .get(&script)
            .is_ok_and(|answer| answer.body.contains("mixengine serves php"))
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "the pool stopped serving after its configuration changed\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let after = status(&home);
    assert_eq!(after["state"], "running", "{after}\n{}", home.daemon_log());
    assert_eq!(
        after["pid"].as_u64(),
        Some(pid),
        "the pool was replaced rather than left alone — which on a system with signals is the cost \
         the whole task avoids, and on one without is a restart nobody asked for: {after}"
    );

    // **The two systems say different things here, and the difference is the assertion.** Unix sent
    // `SIGUSR2` and the master cycled its workers onto the new file; Windows has no signal to send
    // and left the running process on its previous configuration, out loud, in the log. Asserting
    // the second rather than skipping it is what stops a silent regression into "reload did nothing"
    // looking like a pass on the system that reloads.
    let log = home.daemon_log();
    if cfg!(unix) {
        assert!(
            log.contains("re-read its configuration"),
            "no reload was recorded on a system that has signals\n{log}"
        );
    } else {
        // The recipe returns no reload at all on this system rather than one that would be refused,
        // so what the runner says is that there is none — which is the sentence asserted here.
        assert!(
            log.contains("it has no reload, so the running process is still using the previous"),
            "Windows either reloaded something it cannot reload, or said nothing about not \
             having\n{log}"
        );
    }

    // --- stopped, with nothing left holding the endpoint -------------------------------------------
    //
    // The workers are the point: on Unix they are in the master's process group and on Windows they
    // were measured to go with it, and a child left behind is a child the next start collides with.
    let stopped = json(&home.mix(&["service", "stop", &pool, "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    assert!(
        listen.get(&script).is_err(),
        "something is still answering on the pool's endpoint after it was stopped\n{}",
        home.daemon_log()
    );

    // --- and removed with the PHP it ran out of ---------------------------------------------------
    let uninstalled = home.mix(&["runtime", "uninstall", "php", VERSION, "--json"]);
    assert!(
        uninstalled.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&uninstalled.stderr),
        home.daemon_log()
    );

    let remaining = json(&home.mix(&["service", "list", "--json"]));
    assert!(
        remaining["services"]
            .as_array()
            .is_some_and(|services| services.iter().all(|service| service["id"] != pool)),
        "the pool outlived the PHP it ran out of\n{remaining}"
    );
}

/// A PHP that is still serving is not removed out from under itself.
///
/// **The first refusal `runtime.uninstall` has ever been able to make**, which is why it gets its
/// own test rather than a line in the one above: what it needs is a *running* pool, and that test
/// ends with a stopped one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn a_running_pool_refuses_to_have_its_php_removed() {
    let (home, _daemon, _registry, _listen) = installed().await;
    let pool = pool();

    let started = json(&home.mix(&["service", "start", &pool, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let refused = home.mix(&["runtime", "uninstall", "php", VERSION, "--json"]);
    assert!(
        !refused.status.success(),
        "a PHP was removed out from under the pool serving out of it\n{}",
        home.daemon_log()
    );

    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains(&pool) && said.contains("stop"),
        "a refusal has to name the thing in the way and the command that clears it: {said}"
    );

    // The other half, and the one a refusal that half-happened would fail: nothing was removed.
    let runtimes = json(&home.mix(&["runtime", "list", "--json"]));
    assert!(
        runtimes.to_string().contains(VERSION),
        "the runtime was removed by a call that refused: {runtimes}"
    );

    home.mix(&["service", "stop", &pool, "--json"]);
}

/// **What the whole of T72a's reading rule rests on**, proved against the program rather than
/// argued: a status probe costs the pool exactly one `accepted conn`, and an otherwise idle pool
/// reports exactly one active process — the worker answering the probe itself.
///
/// `idle::observe` subtracts precisely that one, so if php-fpm ever changes either number this suite
/// goes red and the daemon's arithmetic is wrong. It is the reason the rule can be a comparison
/// against `+ 1` rather than a guess.
///
/// **Unix only, and it is the recipe's own split**: Windows runs `php-cgi.exe`, which is not php-fpm,
/// reads no pool file and publishes no status page — a pool there is measured by counting
/// connections to a real port, which needs none of this.
///
/// **Run against 7.0.33 as well as against the version this suite pins**, by pointing
/// `MIXENGINE_PHP_RUNTIME` at the older package: green on both. That is the evidence for T72a's
/// decision to use `pm.status_path` rather than the cleaner `pm.status_listen`, which exists only
/// from PHP 8.0 and would have made a 7.x pool refuse to start. `cold_path.rs` is where two old
/// versions are measured on every CI run rather than by hand.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn a_status_probe_costs_the_pool_exactly_one_connection() {
    let (home, _daemon, _registry, listen) = installed().await;

    let started = json(&home.mix(&["service", "start", &pool(), "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let first = numbers(&listen, &home);
    let second = numbers(&listen, &home);

    assert_eq!(
        second.accepted,
        first.accepted + 1,
        "a probe is one request and no more, which is exactly what the daemon subtracts: {first:?} \
         then {second:?}\n{}",
        home.daemon_log()
    );
    assert_eq!(
        second.active, 1,
        "an idle pool answering a status request has one worker busy — itself: {second:?}"
    );
    assert_eq!(
        second.started, first.started,
        "the pool restarted between the two readings, so this proved nothing: {first:?} then \
         {second:?}"
    );

    home.mix(&["service", "stop", &pool(), "--json"]);
}

/// The three numbers `mixengine_supervisor::idle::observe` reads, off a real status page.
#[cfg(unix)]
#[derive(Debug)]
struct Numbers {
    accepted: u64,
    active: u64,
    started: u64,
}

/// One reading, through the same request the daemon makes.
#[cfg(unix)]
fn numbers(listen: &Pool, home: &Home) -> Numbers {
    let answer = listen.status("/mixengine-status").unwrap_or_else(|error| {
        panic!(
            "a pool answers its own status page: {error}\n{}",
            home.daemon_log()
        )
    });

    let document: Value = serde_json::from_str(&answer.body).unwrap_or_else(|_| {
        panic!(
            "the status page answered json — a 404 here is a `pm.status_path` the pool never got: \
             {}",
            answer.body
        )
    });

    let read = |field: &str| {
        document[field]
            .as_u64()
            .unwrap_or_else(|| panic!("`{field}` is a number: {document}"))
    };

    Numbers {
        accepted: read("accepted conn"),
        active: read("active processes"),
        started: read("start time"),
    }
}
