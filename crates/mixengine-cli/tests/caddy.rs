//! The Caddy recipe against a **real** Caddy — roadmap task **T31**.
//!
//! Everything else about this recipe is provable in one process and is proved there: the template
//! renders, the settings merge, the spec builds, the registry hands a changed rendering to a running
//! service. None of that says the thing the task is actually about, which is that *Caddy accepts
//! what MixEngine generates and does what MixEngine asks it to*. That claim can only be made against
//! the program, so this suite is made against the program.
//!
//! **It is `#[ignore]`d rather than skipped**, and the difference is the point. A test that quietly
//! returns when it cannot find a Caddy is a green suite that proved nothing on the day the download
//! broke; an ignored one is *visibly* not run, and the `caddy` step in `.github/workflows/ci.yml`
//! fetches a real archive on all three systems and runs it. Without one, everything here panics
//! saying so.
//!
//! **The whole of T31 in the order a user meets it**: a row becomes a Caddyfile, `caddy validate`
//! judges it, the admin endpoint says when it is up, an edited override is *served* by the same
//! process a moment later, a broken one is refused with the good configuration still live, and
//! `caddy stop` through that endpoint ends it.

mod harness;

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use harness::{Home, json};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked Caddy is, as the CI step and a developer both set it.
///
/// The directory holding the binary — the archive's own root, since `mixengine-packages` publishes
/// Caddy as one executable with nothing around it. That is also what a `packages` row's
/// `install_path` is, which is the whole reason this is a directory and not a path to a file.
const PACKAGE: &str = "MIXENGINE_CADDY_PACKAGE";

/// The version the index publishes this as, and the one `mix service create` names.
///
/// Nothing compares it against what the binary reports — a recipe is found by `packages.name` — but
/// an index entry has to say something, and saying the wrong thing in a message is worse than saying
/// nothing.
const VERSION: &str = "2.x";

/// How long the server is given to be serving something new after a reload.
///
/// Long for what it covers, because what it is really waiting for is a runner's next turn plus a
/// `caddy reload` on a runner that may be compiling something else at the same time.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// The Caddy this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no Caddy to judge this recipe against. The `caddy` \
             step in .github/workflows/ci.yml fetches one; by hand, unpack any Caddy 2.x and point \
             {PACKAGE} at the directory holding the binary."
        )
    });

    let directory = PathBuf::from(directory);
    let binary = directory.join(format!("caddy{}", std::env::consts::EXE_SUFFIX));

    assert!(
        binary.is_file(),
        "{PACKAGE} is {}, which holds no caddy binary",
        directory.display()
    );

    directory
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price: between the drop and Caddy's bind, another process on the
/// machine could take it. Nothing better exists — the alternative is a fixed port, which two runs of
/// this suite on one machine would fight over.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// `GET /` on a loopback port, as raw as it can be, and whatever came back.
///
/// A hand-written request rather than an HTTP client, because what is being asked is small enough
/// that a client would be the bigger thing to get wrong: one connection, one request, read until the
/// server closes. `Connection: close` is what makes the read terminate.
///
/// **The `Host` header has to be the site's own address.** A Caddyfile block written as
/// `http://127.0.0.1:8080` matches on that host, so a request carrying any other one reaches a
/// listening Caddy and is answered `404` — which reads as a reload that did not happen and is a
/// header that did not match.
fn get(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read deadline");

    stream
        .write_all(
            format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;

    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;

    Some(answer)
}

/// A home with a real Caddy **installed** in it, on ports nothing else is using, and a daemon over it.
///
/// The archive is packed here out of the Caddy `.github/workflows/ci.yml` fetched, served by a
/// registry that signs its own index, and installed through `package.install` — so this suite covers
/// the whole T31a path against a real artifact on all three systems at no extra cost, and the
/// service it then creates is one `service.create` wrote rather than one a fixture inserted.
///
/// The admin port is overridden away from Caddy's own 2019 for the reason the site port is chosen
/// rather than fixed: a developer running this suite may well have a Caddy of their own on 2019, and
/// a test that took it over would be a test that stops somebody's work.
async fn declared() -> (Home, harness::Daemon, MockRegistry, u16, u16) {
    let (site, admin) = (free_port(), free_port());

    let packing = match cfg!(windows) {
        true => Packing::Zip,
        false => Packing::TarZst,
    };
    let binary = format!("caddy{}", std::env::consts::EXE_SUFFIX);
    let packed = FakePackage::new(packing)
        .program(&binary, &package().join(&binary))
        .build(&format!("caddy-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-19T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, &binary));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed = json(&home.mix(&["package", "install", "caddy", VERSION, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}
{}",
        home.daemon_log()
    );

    // **No `@`**, which is the instancing rule seen from the recipe that has it: there is one Caddy,
    // and an id carrying an instance name would be refused here.
    let created = json(&home.mix(&[
        "service",
        "create",
        "caddy",
        VERSION,
        "--port",
        &site.to_string(),
        "--json",
    ]));
    assert_eq!(
        created["id"],
        "caddy",
        "{created}
{}",
        home.daemon_log()
    );

    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        "caddy",
        &format!(r#"{{"admin_port": {admin}}}"#),
    )
    .await;

    (home, daemon, registry, site, admin)
}

/// An index offering exactly this Caddy, for this machine.
fn index(packed: &Packed, url: &str, binary: &str) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [{
            "kind": "caddy",
            "version": VERSION,
            "channel": "stable",
            "artifacts": [{
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "url": url,
                "sha256": packed.sha256,
                "size": packed.size(),
                "provides": { "caddy": binary },
            }],
        }],
    })
}

/// What `mix service status caddy` says.
fn status(home: &Home) -> Value {
    json(&home.mix(&["service", "status", "caddy", "--json"]))
}

/// The whole overrides document for a Caddy on `admin` serving one site on `port`.
///
/// **The whole document and not a patch**, which is what `config_overrides_json` is: a setting that
/// is not in it is not set. So every override this suite writes repeats the admin port, and one that
/// forgot would move the endpoint back to Caddy's default under a server listening on the one this
/// home chose — a reload and a stop sent to an address nothing answers on.
fn serving(admin: u16, port: u16, says: &str) -> String {
    serde_json::json!({
        "admin_port": admin,
        "extra": format!("http://127.0.0.1:{port} {{\n\trespond \"{says}\"\n}}\n"),
    })
    .to_string()
}

/// **The whole of T31, in the order a user meets it.**
///
/// One test rather than five, deliberately: each step is the previous one's precondition, and five
/// tests would be five real Caddy servers started to re-reach the state this one is already in. What
/// each assertion proves is written beside it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Caddy — see the module note, and the `caddy` step in ci.yml"]
async fn caddy_is_generated_validated_started_reloaded_and_stopped() {
    let (home, _daemon, _registry, site_port, admin) = declared().await;

    // --- generated, and judged by Caddy itself -------------------------------------------------
    //
    // `service start` renders the Caddyfile and runs `caddy validate` over the staged copy before
    // installing it, so a start that completes is a configuration the real adapter accepted.
    let started = json(&home.mix(&["service", "start", "caddy", "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let caddyfile = home.path().join("etc").join("caddy").join("Caddyfile");
    let rendered = std::fs::read_to_string(&caddyfile).expect("the generated Caddyfile");
    assert!(
        rendered.contains(&format!("admin 127.0.0.1:{admin}")),
        "{rendered}"
    );

    // --- started, and proved up by the admin endpoint -------------------------------------------
    //
    // The readiness check in the spec is `GET /config/` on that endpoint, so a service the daemon
    // reports as running is one that answered it.
    let up = status(&home);
    assert_eq!(up["state"], "running", "{up}\n{}", home.daemon_log());
    let pid = up["pid"].as_u64().expect("a running service has a pid");

    assert!(
        get(site_port).is_none(),
        "a Caddy with no sites answered on the port sites are served on"
    );

    // --- reloaded ------------------------------------------------------------------------------
    //
    // A site pasted into the free-form override, and then nothing but a listing: the configuration
    // is rendered at the top of every `service.*` call, and a rendering that moved under a running
    // service is handed to it. Nothing here restarts anything.
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        "caddy",
        &serving(admin, site_port, "mixengine reloaded me"),
    )
    .await;

    let listed = json(&home.mix(&["service", "list", "--json"]));
    assert_eq!(listed["services"][0]["state"], "running", "{listed}");

    let deadline = Instant::now() + EVENTUALLY;
    loop {
        if get(site_port).is_some_and(|answer| answer.contains("mixengine reloaded me")) {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "the running Caddy never served the site the reload gave it\n--- Caddyfile ---\n{}\n\
             --- daemon.log ---\n{}",
            std::fs::read_to_string(&caddyfile).unwrap_or_default(),
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let reloaded = status(&home);
    assert_eq!(
        reloaded["pid"].as_u64(),
        Some(pid),
        "the server was replaced rather than reloaded, which is the cost the whole task avoids: \
         {reloaded}"
    );

    // --- refused, with the last good configuration still live ------------------------------------
    //
    // The half of validation that matters. `caddy validate` refuses the staged rendering, so nothing
    // is installed — and the process goes on serving what it was serving, which is what a user whose
    // typo would otherwise have taken every site on the machine down needs to be true.
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        "caddy",
        &format!(r#"{{"admin_port": {admin}, "extra": "this is not a Caddyfile {{"}}"#),
    )
    .await;

    let refused = home.mix(&["service", "list", "--json"]);
    assert!(
        !refused.status.success(),
        "a Caddyfile Caddy cannot parse was accepted: {}",
        String::from_utf8_lossy(&refused.stdout)
    );

    assert!(
        std::fs::read_to_string(&caddyfile)
            .expect("the Caddyfile is still there")
            .contains("mixengine reloaded me"),
        "the refused rendering was installed anyway"
    );
    assert!(
        get(site_port).is_some_and(|answer| answer.contains("mixengine reloaded me")),
        "a configuration that was never installed stopped the site that was being served"
    );

    // --- stopped, through the admin endpoint -----------------------------------------------------
    //
    // `caddy stop --address`, which is the spec's `StopBehaviour::Command`. The port going quiet is
    // what says the process really went, rather than the row having been written.
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        "caddy",
        &serving(admin, site_port, "mixengine reloaded me"),
    )
    .await;

    let stopped = json(&home.mix(&["service", "stop", "caddy", "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );
    assert_eq!(status(&home)["state"], "stopped");

    let deadline = Instant::now() + EVENTUALLY;
    while get(site_port).is_some() {
        assert!(
            Instant::now() < deadline,
            "something is still serving the site port after Caddy was stopped\n--- daemon.log ---\n{}",
            home.daemon_log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
