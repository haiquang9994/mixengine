//! The Memcached recipe against a **real** memcached — roadmap task **T35**.
//!
//! `#[ignore]`d rather than skipped, for `caddy.rs`' reason: a test that quietly returns when it
//! cannot find a memcached is a green suite that proved nothing on the day the download broke. The
//! `memcached` step in `.github/workflows/ci.yml` fetches a real archive; without one, everything
//! here panics saying so.
//!
//! # This suite speaks the protocol itself, and has to
//!
//! Every other recipe's end-to-end suite asks the server a question through a client the package
//! published — `redis-cli ping`, `mariadb-admin ping`, `psql -tAc`. **This package is one file.**
//! `bin/memcached` is the whole archive, so there is nothing to ask with and the text protocol is
//! written out here: `version`, then `set` and `get`. That is the same thing `mixengine-packages`
//! does to smoke-test the artifact, and for the same reason.
//!
//! # The claim only a real run can settle
//!
//! **The configuration is the command line.** This is the one recipe in the catalogue that renders
//! no file, so what would be a rendering to read back is instead a server that has to *behave* as
//! the flags say — listening where the row put it, and reachable there. A memcached given a flag it
//! does not understand exits rather than falling back, which is what makes the start itself the
//! check that every flag this recipe writes is one this build accepts.

mod harness;

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use harness::{Home, json};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked memcached is, as the CI step and a developer both set it.
const PACKAGE: &str = "MIXENGINE_MEMCACHED_PACKAGE";

/// The version the index publishes this as, and the one `mix service create` names.
const VERSION: &str = "1.6";

/// The service this suite drives. **An `@`**: a home may hold two caches, so every one is named.
const SERVICE: &str = "memcached@main";

/// How long the server is given to stop answering after it has been asked to.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// The memcached this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no memcached to judge this recipe against. The \
             `memcached` step in .github/workflows/ci.yml fetches one; by hand, unpack any \
             memcached 1.6 from mixengine-packages' releases and point {PACKAGE} at the directory \
             it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes — one program, and the archive is nothing else.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let relative = Path::new("bin").join(format!("memcached{}", std::env::consts::EXE_SUFFIX));

    assert!(
        root.join(&relative).is_file(),
        "{PACKAGE} is {}, which holds no {}",
        root.display(),
        relative.display()
    );

    [(
        "memcached".to_owned(),
        Value::String(relative.to_string_lossy().replace('\\', "/")),
    )]
    .into_iter()
    .collect()
}

/// A conversation in memcached's text protocol, and everything that came back.
///
/// `quit` is what makes the read terminate: the server closes the connection on it, and without one
/// this would sit on the read deadline for every exchange.
fn say(port: u16, lines: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read deadline");

    stream
        .write_all(format!("{lines}\r\nquit\r\n").as_bytes())
        .ok()?;

    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;

    Some(answer)
}

/// The same, insisting there was an answer.
fn said(port: u16, lines: &str) -> String {
    say(port, lines).unwrap_or_else(|| panic!("nothing answered `{lines}` on {port}"))
}

/// Wait for `wanted` to hold, or say what it was still doing when the deadline passed.
fn eventually(what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + EVENTUALLY;

    while !wanted() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// An index offering exactly this memcached, for this machine.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-20T06:55:12Z",
        "packages": [{
            "kind": "memcached",
            "version": VERSION,
            "channel": "stable",
            "artifacts": [{
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "url": url,
                "sha256": packed.sha256,
                "size": packed.size(),
                "provides": provides,
            }],
        }],
    })
}

/// A home with a real memcached installed in it, a service created against it, and a daemon over
/// both.
async fn created() -> (Home, harness::Daemon, MockRegistry, u16) {
    let root = package();
    let port = free_port();

    let packing = if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    };
    let packed = FakePackage::new(packing)
        .directory(&root)
        .build(&format!("memcached-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-20T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed = json(&home.mix(&["package", "install", "memcached", VERSION, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

    let created = json(&home.mix(&[
        "service",
        "create",
        SERVICE,
        VERSION,
        "--port",
        &port.to_string(),
        "--json",
    ]));
    assert_eq!(
        created["service"]["id"],
        SERVICE,
        "{created}\n{}",
        home.daemon_log()
    );

    (home, daemon, registry, port)
}

/// **The whole of T35's memcached half, in the order a user meets it.**
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real memcached — see the module note, and the `memcached` step in ci.yml"]
async fn a_cache_with_no_configuration_file_is_started_written_to_and_stopped() {
    let (home, _daemon, _registry, port) = created().await;

    // --- started, on flags alone -----------------------------------------------------------------
    //
    // A memcached given a flag it does not understand exits rather than ignoring it, so a start that
    // completes is a command line this build accepted — which is the only validation this recipe
    // has and the reason it needs none of T30's.
    let started = json(&home.mix(&["service", "start", SERVICE, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    // --- and it rendered nothing, which is a claim about the disk ---------------------------------
    //
    // The one service in the catalogue with no `etc/<service-id>/` at all. Asserting `files()` is
    // empty proves a list is empty; this proves nobody created a directory around it either.
    let etc = home.path().join("etc").join(SERVICE);
    assert!(
        !etc.exists(),
        "memcached reads no configuration file, so {} should not exist",
        etc.display()
    );

    // --- it is this server, on the port the row chose ---------------------------------------------
    let version = said(port, "version");
    assert!(version.contains("VERSION"), "{version}");

    // --- and it caches ----------------------------------------------------------------------------
    let stored = said(port, "set greeting 0 0 5\r\nhello");
    assert!(stored.contains("STORED"), "{stored}");

    let read_back = said(port, "get greeting");
    assert!(read_back.contains("hello"), "{read_back}");
    assert!(read_back.contains("END"), "{read_back}");

    // --- stopped by being killed, which is what ADR 0008 says about this service -------------------
    let stopped = json(&home.mix(&["service", "stop", SERVICE, "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    eventually("the server went on answering after it was stopped", || {
        say(port, "version").is_none()
    });

    let status = json(&home.mix(&["service", "status", SERVICE, "--json"]));
    assert_eq!(
        status["state"],
        "stopped",
        "{status}\n{}",
        home.daemon_log()
    );
}
