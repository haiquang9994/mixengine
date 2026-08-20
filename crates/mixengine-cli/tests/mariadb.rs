//! The MariaDB recipe against a **real** MariaDB — roadmap task **T33**.
//!
//! Everything else about this recipe is provable in one process and is proved there: the template
//! renders with every path quoted, the settings merge, the spec builds, the file and the readiness
//! check name one port, the bootstrap SQL says what it does. None of that says the thing the task is
//! about, which is that *a data directory MixEngine bootstrapped becomes a database that answers a
//! query as the root it generated a password for*. That claim can only be made against the server.
//!
//! **It is `#[ignore]`d rather than skipped**, for `caddy.rs`' reason: a test that quietly returns
//! when it finds no MariaDB is a green suite that proved nothing on the day the download broke.
//!
//! **This suite needs a credential store**, which is the one way it differs from the other two: the
//! root password has exactly one home, and a machine with none refuses the bootstrap by design. On
//! Windows and macOS the store is part of the OS; on Linux `.github/scripts/test-no-network.sh`
//! starts a `gnome-keyring` on a session bus of its own, which is why the Linux leg runs this from
//! inside that script rather than beside it.

mod harness;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use harness::{Home, json};
use mixengine_platform::KEYRING_SERVICE;
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// How long the whole of this suite is given before a hang is reported as the hang it is.
///
/// Twelve minutes, against a Linux run that takes thirty-six seconds and a Windows one that takes
/// four minutes with a cold Defender in the way. It is not a budget anything is expected to come
/// near — it is the line past which *waiting* stops being the answer.
const BUDGET: Duration = Duration::from_secs(720);

/// What this suite is doing, for the thread that has to report a hang.
static STAGE: Mutex<&'static str> = Mutex::new("packing the archive and starting a daemon");

/// Say what is happening now, in the log and to the watchdog.
fn at(stage: &'static str) {
    *STAGE.lock().expect("nothing panics holding this") = stage;
    eprintln!("[mariadb] {stage}");
}

/// Turn a hang into a failure that says where it hung.
///
/// **Measured, and it cost a whole CI run.** The macOS leg of the first run of this suite spent
/// twenty-seven minutes inside it and was killed by the job's own timeout, having printed *nothing*
/// — because libtest holds a running test's output until the test ends, and this one never ended.
/// A thread is what reports it rather than a `tokio::time::timeout`, and the difference is the one
/// that matters here: the calls this suite makes are blocking ones — a client process, a keyring
/// round trip — and an async deadline around a blocked thread never fires. libtest's capture is
/// thread-local, so what this thread prints reaches the log even while the test's own output is
/// still being held.
fn watch(home: &Home) {
    let log = home.path().join("logs").join("daemon.log");

    std::thread::spawn(move || {
        std::thread::sleep(BUDGET);

        let stage = *STAGE.lock().expect("nothing panics holding this");

        eprintln!(
            "\n--- this suite hung ---\nIt was {stage}, and it had been for less than {BUDGET:?}.             \n--- daemon.log ---\n{}",
            std::fs::read_to_string(&log)
                .unwrap_or_else(|error| format!("{} could not be read: {error}", log.display()))
        );

        // The process rather than the thread: a test blocked in a syscall cannot be unwound, and
        // the only thing left to decide is whether the job learns why in twelve minutes or in
        // thirty.
        std::process::exit(101);
    });
}

/// Where an unpacked MariaDB is, as the CI step and a developer both set it.
///
/// The directory the archive unpacks to — `bin/mariadbd` inside it — which is also what a `packages`
/// row's `install_path` is.
const PACKAGE: &str = "MIXENGINE_MARIADB_PACKAGE";

/// The version the index publishes it as, and the one `mix service create` names.
const VERSION: &str = "11.4.12";

/// The service this suite drives. **An `@`**, which is the instancing rule seen from a recipe that
/// has it: a home may hold two databases, so every one of them is named.
const SERVICE: &str = "mariadb@main";

/// The credential the recipe declares, at the address it composes.
const CREDENTIAL: &str = "mariadb@main/root";

/// The MariaDB this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no MariaDB to judge this recipe against. The \
             `mariadb` step in .github/workflows/ci.yml fetches one; by hand, unpack any MariaDB \
             11.4 from mixengine-packages' releases and point {PACKAGE} at the directory it \
             unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and it is worth paying here rather than fixing on 3306: a
/// developer running this suite very likely has a MariaDB of their own, and a test that took its
/// port would be a test that stops somebody's work.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes, as an index entry says it.
///
/// **Probed rather than written down**, because the layout is the publisher's and upstream renamed
/// every one of these between 10.4 and 10.6 — a suite that hard-coded either spelling would pass on
/// one series while describing the other wrongly.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for (name, candidates) in [
        ("mariadbd", ["bin/mariadbd", "bin/mysqld"].as_slice()),
        ("mariadb", ["bin/mariadb", "bin/mysql"].as_slice()),
        (
            "mariadb-admin",
            ["bin/mariadb-admin", "bin/mysqladmin"].as_slice(),
        ),
        (
            "mariadb-install-db",
            [
                "scripts/mariadb-install-db",
                "bin/mariadb-install-db",
                "scripts/mysql_install_db",
                "bin/mysql_install_db",
            ]
            .as_slice(),
        ),
    ] {
        for candidate in candidates {
            let relative = format!("{candidate}{}", std::env::consts::EXE_SUFFIX);

            if root.join(&relative).is_file() {
                found.insert(name.to_owned(), Value::String(relative));
                break;
            }
        }

        // Named here rather than left to the recipe: `ServiceProvidesNothing` arriving from three
        // layers down at `service start` says the same thing much later and much less clearly.
        assert!(
            found.contains_key(name),
            "{} publishes no {name}, so this suite has nothing to drive — it needs the MariaDB \
             mixengine-packages builds, not a system one",
            root.display()
        );
    }

    found
}

/// An index offering exactly this MariaDB, for this machine.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [{
            "kind": "mariadb",
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

/// Where this instance's data directory is: `data/<package>/<instance>`, because it is named.
fn data_directory(home: &Home) -> PathBuf {
    home.path().join("data").join("mariadb").join("main")
}

/// `mix …` for a call that is expected to work, with the daemon's own log in the failure.
///
/// [`harness::json`] panics on a non-zero exit with only `mix`'s stderr, which for a start that
/// failed is one sentence about a service; what says *why* is `daemon.log`, and a bootstrap that
/// went wrong is the case where the difference is an afternoon.
fn expect(home: &Home, args: &[&str]) -> Value {
    let output = home.mix(args);

    assert!(
        output.status.success(),
        "`mix {}` exited {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n--- daemon.log ---\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        home.daemon_log()
    );

    json(&output)
}

/// What `mix service status mariadb@main` says.
fn status(home: &Home) -> Value {
    json(&home.mix(&["service", "status", SERVICE, "--json"]))
}

/// Every `service.first_run` job this home has run.
fn first_runs(home: &Home) -> Vec<Value> {
    let listed = json(&home.mix(&["job", "list", "--json"]));

    listed["jobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|job| job["kind"] == "service.first_run")
        .collect()
}

/// The generated root password, read back out of the OS credential store.
///
/// **Through the keyring rather than through anything of ours**, which is the point: what is being
/// proved is that the value the ritual stored is the value the server was given, and reading it any
/// other way would be reading our own copy of it.
fn root_password() -> String {
    at("reading the generated root password back out of the OS credential store");

    mixengine_platform::host()
        .keyring()
        .secret(KEYRING_SERVICE, CREDENTIAL)
        .expect("this machine has a credential store")
        .expect("the ritual stored a root password")
}

/// Ask the server a question, as root, with the password from the keyring.
///
/// The shipped client rather than a Rust driver: the workspace has no MySQL protocol implementation
/// and has no reason to grow one for a test, and the client is in the archive this suite installed.
fn query(root: &Path, port: u16, sql: &str) -> std::process::Output {
    let client = root.join(format!("bin/mariadb{}", std::env::consts::EXE_SUFFIX));
    let client = if client.is_file() {
        client
    } else {
        root.join(format!("bin/mysql{}", std::env::consts::EXE_SUFFIX))
    };

    Command::new(client)
        .args([
            "--protocol=TCP",
            "--host=127.0.0.1",
            &format!("--port={port}"),
            "--user=root",
            "--batch",
            "--skip-column-names",
            "-e",
            sql,
        ])
        // The one way a password reaches a MariaDB client without being on a command line every
        // process on the machine can read — the same variable the recipe's spec names.
        .env("MYSQL_PWD", root_password())
        .output()
        .expect("the client in the archive can be run")
}

/// A home with a real MariaDB installed in it, a service created over it, and the port it will use.
///
/// The archive is packed here out of the directory the CI step unpacked, served by a registry that
/// signs its own index, and installed through `package.install` — so this suite covers the whole
/// package install path against a real artifact on all three systems at no extra cost.
async fn created() -> (Home, harness::Daemon, MockRegistry, PathBuf, u16) {
    let root = package();
    let port = free_port();

    let packing = if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    };
    let packed = FakePackage::new(packing)
        .directory(&root)
        .build(&format!("mariadb-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-19T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());
    watch(&home);

    at("installing the package");
    let installed = expect(&home, &["package", "install", "mariadb", VERSION, "--json"]);
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

    at("creating the service");
    let created = expect(
        &home,
        &[
            "service",
            "create",
            SERVICE,
            VERSION,
            "--port",
            &port.to_string(),
            "--json",
        ],
    );
    assert_eq!(created["id"], SERVICE, "{created}\n{}", home.daemon_log());

    // The install path this home unpacked into, which is where the client comes from.
    let installed_at = home.path().join("packages").join("mariadb").join(VERSION);

    (home, daemon, registry, installed_at, port)
}

/// **The whole of T33, in the order a user meets it.**
///
/// One test rather than eight, deliberately: each step is the previous one's precondition, and eight
/// tests would be eight real bootstraps performed to re-reach the state this one is already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real MariaDB — see the module note, and the `mariadb` step in ci.yml"]
async fn a_database_is_bootstrapped_started_queried_stopped_and_not_bootstrapped_twice() {
    let (home, _daemon, _registry, installed_at, port) = created().await;
    let data = data_directory(&home);

    // --- started, which is where the bootstrap happens -------------------------------------------
    //
    // Nothing here asked for a first run. The start did, because the data directory is empty — which
    // is the half of T33 that is invisible from inside the daemon and obvious from out here.
    at("starting the service, which is where the first run bootstraps");
    let started = expect(&home, &["service", "start", SERVICE, "--json"]);
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let jobs = first_runs(&home);
    assert_eq!(
        jobs.len(),
        1,
        "a start with an empty data directory ran {} first-run jobs\n{}",
        jobs.len(),
        home.daemon_log()
    );
    assert_eq!(
        jobs[0]["state"],
        "succeeded",
        "{}\n{}",
        jobs[0],
        home.daemon_log()
    );

    // --- and left a data directory that says who made it -----------------------------------------
    let marker = std::fs::read_to_string(data.join(".mixengine-ready"))
        .expect("a finished ritual leaves its marker");
    assert_eq!(marker.trim(), VERSION, "the marker names another version");
    assert!(
        data.join("mysql").is_dir(),
        "the bootstrap left no system schema in {}",
        data.display()
    );

    // --- running, and proved so by a query rather than by an accept -------------------------------
    let up = status(&home);
    assert_eq!(up["state"], "running", "{up}\n{}", home.daemon_log());

    // **The assertion this whole suite exists for.** A server that has accepted a connection and is
    // still recovering answers a TCP probe exactly like one that works, so the claim is made by
    // running a query as the root whose password the ritual generated and stored.
    at("asking the server for its version, as root");
    let answered = query(&installed_at, port, "SELECT VERSION();");
    let said = String::from_utf8_lossy(&answered.stdout);
    assert!(
        answered.status.success(),
        "the server did not answer a query as root: {said}{}\n{}",
        String::from_utf8_lossy(&answered.stderr),
        home.daemon_log()
    );
    assert!(
        said.trim().starts_with(VERSION),
        "the server that answered is not the one installed: {said}"
    );

    // The anonymous accounts the bootstrap removed are gone, which is the other half of what that
    // step is for — and is a claim about the grant tables rather than about the statement that made
    // them.
    let anonymous = query(
        &installed_at,
        port,
        "SELECT COUNT(*) FROM mysql.global_priv WHERE User = '';",
    );
    assert_eq!(
        String::from_utf8_lossy(&anonymous.stdout).trim(),
        "0",
        "the bootstrap left an anonymous account behind"
    );

    // --- stopped, cleanly ------------------------------------------------------------------------
    at("stopping the service");
    let stopped = expect(&home, &["service", "stop", SERVICE, "--json"]);
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    // **Proof, rather than a belief about an exit code.** A `mariadbd` that was terminated exits
    // zero and leaves a dirty buffer pool; only the server's own log says the shutdown finished.
    let log = home
        .path()
        .join("logs")
        .join("services")
        .join(SERVICE)
        .join("mariadb.err");
    let said = std::fs::read_to_string(&log)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", log.display()));
    assert!(
        said.contains("Shutdown complete"),
        "the server was stopped rather than asked to shut down:\n{said}"
    );

    // --- started a second time, and not bootstrapped again ----------------------------------------
    at("starting it a second time, which must not bootstrap again");
    let again = expect(&home, &["service", "start", SERVICE, "--json"]);
    assert_eq!(again["complete"], true, "{again}\n{}", home.daemon_log());

    assert_eq!(
        first_runs(&home).len(),
        1,
        "a second start bootstrapped the data directory again\n{}",
        home.daemon_log()
    );

    let still = query(&installed_at, port, "SELECT VERSION();");
    assert!(
        still.status.success(),
        "the credential did not survive a restart: {}\n{}",
        String::from_utf8_lossy(&still.stderr),
        home.daemon_log()
    );

    // --- and a data directory that is not ours is refused, not cleaned ----------------------------
    //
    // The markers are what make a directory ours, so removing them is exactly what a user pointing
    // this service at a database they already had looks like from in here.
    let stopped = expect(&home, &["service", "stop", SERVICE, "--json"]);
    assert_eq!(stopped["complete"], true, "{stopped}");

    std::fs::remove_file(data.join(".mixengine-ready")).expect("the marker is there");
    // Beside the directory rather than inside it — see `first_run::STARTED_MARKER`, and the Windows
    // bootstrapper that will not touch a datadir with anything at all in it.
    let _ = std::fs::remove_file(data.with_file_name("main.mixengine-init-started"));

    // `expect` would be wrong here: this call is *meant* to fail, and what is asserted is the exit
    // status rather than a field — the walk answers `"complete": true` for a plan it finished
    // walking, and names the service it could not start under `failed`.
    at("starting over a data directory MixEngine did not create");
    let refused = home.mix(&["service", "start", SERVICE, "--json"]);
    let said = harness::stdout(&refused);
    assert!(
        !refused.status.success(),
        "a database MixEngine did not create was bootstrapped over\n{said}\n{}",
        home.daemon_log()
    );
    assert!(
        said.contains("was not created by MixEngine"),
        "the start failed for some other reason: {said}"
    );
    assert!(
        data.join("mysql").is_dir(),
        "the refusal removed somebody's database"
    );
}
