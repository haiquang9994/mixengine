//! The Redis recipe against a **real** Redis — roadmap task **T35**.
//!
//! Everything else about this recipe is provable in one process and is proved there: the template
//! renders, the settings merge, the spec builds. None of that says the thing the task is about,
//! which is that *Redis accepts what MixEngine generates and does what MixEngine asks it to*. That
//! claim can only be made against the program, so this suite is made against the program.
//!
//! **It is `#[ignore]`d rather than skipped**, for `caddy.rs`' reason: a test that quietly returns
//! when it cannot find a Redis is a green suite that proved nothing on the day the download broke.
//! The `redis` step in `.github/workflows/ci.yml` fetches a real archive; without one, everything
//! here panics saying so.
//!
//! # The two claims only a real server can settle
//!
//! **The configuration is found at all.** `getAbsolutePath()` in Redis's `server.c` joins any path
//! not starting with `/` to `getcwd()`, so the recipe names `redis.conf` relatively beside a working
//! directory of its own — and whether that arrangement works is a question about the server's own
//! path handling on the system it is running on. A server started with a configuration it did not
//! find does not fail: it starts on **6379** with Redis's built-in defaults, which is a different
//! server on a different port and would answer a check written any less carefully than the one here.
//!
//! **The cache keeps nothing.** `save ""`, `appendonly no` and `SHUTDOWN NOSAVE` are three
//! statements in three places, and what makes them one behaviour is a key written, a restart, and
//! the key being gone. Asserting the template contains `save ""` proves the template contains
//! `save ""`.

mod harness;

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use harness::{Home, json};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked Redis is, as the CI step and a developer both set it.
const PACKAGE: &str = "MIXENGINE_REDIS_PACKAGE";

/// The version the index publishes this as, and the one `mix service create` names.
const VERSION: &str = "8.x";

/// The service this suite drives. **An `@`**: a home may hold two caches, so every one is named.
const SERVICE: &str = "redis@main";

/// How long the server is given to stop answering after it has been asked to.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// The Redis this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no Redis to judge this recipe against. The `redis` \
             step in .github/workflows/ci.yml fetches one; by hand, unpack any Redis from \
             mixengine-packages' releases and point {PACKAGE} at the directory it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and it is worth paying here rather than fixing on 6379: a
/// developer running this suite may well have a Redis of their own, and a test that took its port
/// would be a test that stops somebody's work. It is also what makes the check below meaningful —
/// see the module note about a server that did not find its configuration.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes, as an index entry says it.
///
/// Probed rather than written down: the layout is the publisher's, and `.exe` is only half of what
/// differs between the cells.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for name in ["redis-server", "redis-cli"] {
        let relative = Path::new("bin").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));

        assert!(
            root.join(&relative).is_file(),
            "{PACKAGE} is {}, which holds no {}",
            root.display(),
            relative.display()
        );

        found.insert(
            name.to_owned(),
            Value::String(relative.to_string_lossy().replace('\\', "/")),
        );
    }

    found
}

/// One command in Redis's inline protocol, and the raw answer to it.
///
/// A hand-written line rather than a client library, for `caddy.rs`' reason: what is being asked is
/// small enough that a client would be the bigger thing to get wrong. Inline commands are part of
/// the protocol rather than a convenience — a server that answers one is a server, and `redis-cli`
/// being on this machine is not what the question is about.
fn ask(port: u16, command: &str) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a read deadline");

    stream
        .write_all(format!("{command}\r\nQUIT\r\n").as_bytes())
        .ok()?;

    let mut answer = String::new();
    stream.read_to_string(&mut answer).ok()?;

    Some(answer)
}

/// The same, insisting there was an answer.
fn asked(port: u16, command: &str) -> String {
    ask(port, command).unwrap_or_else(|| panic!("nothing answered `{command}` on {port}"))
}

/// Wait for `wanted` to hold, or say what it was still doing when the deadline passed.
fn eventually(what: &str, wanted: impl Fn() -> bool) {
    let deadline = Instant::now() + EVENTUALLY;

    while !wanted() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// An index offering exactly this Redis, for this machine.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-20T06:55:12Z",
        "packages": [{
            "kind": "redis",
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

/// A home with a real Redis installed in it, a service created against it, and a daemon over both.
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
        .build(&format!("redis-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-20T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed = json(&home.mix(&["package", "install", "redis", VERSION, "--json"]));
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

/// **The whole of T35's Redis half, in the order a user meets it.**
///
/// One test rather than five, for `mariadb.rs`' reason: each step is the previous one's
/// precondition, and five tests would be five real servers started to re-reach the state this one is
/// already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Redis — see the module note, and the `redis` step in ci.yml"]
async fn a_cache_is_generated_started_written_to_restarted_empty_and_stopped() {
    let (home, _daemon, _registry, port) = created().await;

    // --- generated and started ------------------------------------------------------------------
    let started = json(&home.mix(&["service", "start", SERVICE, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let config = home.path().join("etc").join(SERVICE).join("redis.conf");
    let rendered = std::fs::read_to_string(&config).expect("the generated redis.conf");
    assert!(rendered.contains(&format!("port {port}")), "{rendered}");

    // --- it is *this* server on *this* port, which is what proves the file was read ---------------
    //
    // A Redis that did not find its configuration starts on 6379 with its own defaults. The port
    // this home chose is one nothing else was listening on a moment ago, so an answer here is an
    // answer from the server MixEngine started, reading the file MixEngine rendered.
    let pong = asked(port, "PING");
    assert!(pong.contains("+PONG"), "{pong}");

    // --- it is a cache, and it works as one -------------------------------------------------------
    let stored = asked(port, "SET greeting hello");
    assert!(stored.contains("+OK"), "{stored}");

    let read_back = asked(port, "GET greeting");
    assert!(read_back.contains("hello"), "{read_back}");

    // --- and it keeps nothing, which is the decision this recipe is built around ------------------
    //
    // Three statements in three places — `save ""`, `appendonly no`, `SHUTDOWN NOSAVE` — become one
    // behaviour here or they do not become one at all.
    let restarted = json(&home.mix(&["service", "restart", SERVICE, "--json"]));
    assert_eq!(
        restarted["complete"],
        true,
        "{restarted}\n{}",
        home.daemon_log()
    );

    eventually("the restarted server never answered a ping", || {
        ask(port, "PING").is_some_and(|answer| answer.contains("+PONG"))
    });

    let gone = asked(port, "GET greeting");
    assert!(
        gone.contains("$-1") || gone.contains("_\r\n"),
        "a cache that survived a restart is one somebody will come to trust: {gone}"
    );

    let data = home.path().join("data").join(SERVICE);
    assert!(
        !data.join("dump.rdb").exists(),
        "nothing should have been written to {}",
        data.display()
    );

    // --- stopped through its own client -----------------------------------------------------------
    let stopped = json(&home.mix(&["service", "stop", SERVICE, "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    eventually("the server went on answering after it was stopped", || {
        ask(port, "PING").is_none()
    });

    let status = json(&home.mix(&["service", "status", SERVICE, "--json"]));
    assert_eq!(
        status["state"],
        "stopped",
        "{status}\n{}",
        home.daemon_log()
    );
}
