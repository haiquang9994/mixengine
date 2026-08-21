//! Two instances of one service, side by side — roadmap task **T36**.
//!
//! Everything a *single* instance needs is settled and proved by `mariadb.rs`. What is left is the
//! claim the vocabulary was built for and nothing had yet made: **a home may hold two of one
//! server, at two versions, and neither knows the other is there.** Every mechanism that claim
//! rests on already keys itself by service id rather than by package — the data directory
//! (`data/<package>/<instance>`), the socket, the log directory, the keyring address, the port a
//! row is given — so this suite is not testing a new feature. It is testing that those five
//! decisions, each made in its own task for its own reason, add up to the one thing they were made
//! for.
//!
//! **Two versions rather than two names.** `mariadb@main` on 11.4.12 and `mariadb@legacy` on
//! 10.6.28: two `packages` rows, two install paths, two `mariadbd` binaries whose `share/` layouts
//! differ, and a bootstrap whose every program upstream renamed between those lines. Two instances
//! of *one* version would share a parent row and prove only that two directories can have two
//! names, which the unit tests already say. The version each server reports in its own log is what
//! makes this suite's central assertion unfakeable: the marker in each data directory names the
//! version that bootstrapped it, and the two are different.
//!
//! **It is `#[ignore]`d rather than skipped**, and it needs a credential store, for `mariadb.rs`'
//! reasons — the module note there is the long version of both, and of why nothing here ever reads
//! a credential back.

mod harness;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use harness::{Home, json};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// How long the whole of this suite is given before a hang is reported as the hang it is.
///
/// Twice `mariadb.rs`' twelve minutes, because this does twice its work: two installs, two
/// bootstraps, two servers. Not a budget anything is expected to come near — the line past which
/// *waiting* stops being the answer.
const BUDGET: Duration = Duration::from_secs(1440);

/// What this suite is doing, for the thread that has to report a hang.
static STAGE: Mutex<&'static str> = Mutex::new("packing two archives and starting a daemon");

/// Say what is happening now, in the log and to the watchdog.
fn at(stage: &'static str) {
    *STAGE.lock().expect("nothing panics holding this") = stage;
    eprintln!("[instances] {stage}");
}

/// Turn a hang into a failure that says where it hung.
///
/// A thread rather than a `tokio::time::timeout`, and libtest's thread-local capture is why it can
/// report anything at all — `mariadb.rs`' `watch` carries the measurement that bought this.
fn watch(home: &Home) {
    let log = home.path().join("logs").join("daemon.log");

    std::thread::spawn(move || {
        std::thread::sleep(BUDGET);

        let stage = *STAGE.lock().expect("nothing panics holding this");

        eprintln!(
            "\n--- this suite hung ---\nIt was {stage}, and it had been for less than \
             {BUDGET:?}.\n--- daemon.log ---\n{}",
            std::fs::read_to_string(&log)
                .unwrap_or_else(|error| format!("{} could not be read: {error}", log.display()))
        );

        // The process rather than the thread: a test blocked in a syscall cannot be unwound.
        std::process::exit(101);
    });
}

/// One of the two MariaDBs this suite runs: a version, where it is unpacked, and what to call it.
struct Line {
    /// The version the index publishes it as, and the one `mix service create` names.
    version: &'static str,

    /// The environment variable the CI step and a developer both set.
    variable: &'static str,

    /// The service id, whose instance half is this line's whole point.
    service: &'static str,

    /// The instance half on its own, which is also the data directory's last component.
    instance: &'static str,
}

/// The current line, which is the one `mariadb.rs` already drives on its own.
const MAIN: Line = Line {
    version: "11.4.12",
    variable: "MIXENGINE_MARIADB_PACKAGE",
    service: "mariadb@main",
    instance: "main",
};

/// The oldest line the index publishes, which is the point: upstream renamed every program the
/// bootstrap runs between this line and the one above, and their `share/` layouts differ.
const LEGACY: Line = Line {
    version: "10.6.28",
    variable: "MIXENGINE_MARIADB_LEGACY_PACKAGE",
    service: "mariadb@legacy",
    instance: "legacy",
};

/// Both lines, in the order a person meets them.
const BOTH: [&Line; 2] = [&MAIN, &LEGACY];

/// Where an unpacked MariaDB of this line is, or the reason there is none.
fn package(line: &Line) -> PathBuf {
    let variable = line.variable;
    let version = line.version;

    let directory = std::env::var_os(variable).unwrap_or_else(|| {
        panic!(
            "{variable} is not set, so there is no MariaDB {version} to run beside the other one. \
             The `mariadb-instances` step in .github/workflows/ci.yml fetches both; by hand, \
             unpack MariaDB {version} from mixengine-packages' releases and point {variable} at \
             the directory it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// Both instances are given one rather than letting the allocator choose: what the allocator does
/// with two rows wanting 3306 is a unit test in `services::ports`, and a suite that took a
/// developer's real 3306 to re-assert it would be a suite that stops somebody's work.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes, as an index entry says it.
///
/// **Probed rather than written down**, and this is where the two lines differ most: 10.6 ships
/// `mysqld`, `mysql`, `mysqladmin` and `scripts/mysql_install_db`, and 11.4 ships the `mariadb-`
/// spellings of all four. A suite that hard-coded either would describe one of its own two servers
/// wrongly — which is the reason this suite runs two *versions*.
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

        assert!(
            found.contains_key(name),
            "{} publishes no {name}, so this suite has nothing to drive — it needs the MariaDB \
             mixengine-packages builds, not a system one",
            root.display()
        );
    }

    found
}

/// One package entry, for one line, for this machine.
fn entry(
    version: &str,
    packed: &Packed,
    url: &str,
    provides: serde_json::Map<String, Value>,
) -> Value {
    serde_json::json!({
        "kind": "mariadb",
        "version": version,
        "channel": "stable",
        "artifacts": [{
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "url": url,
            "sha256": packed.sha256,
            "size": packed.size(),
            "provides": Value::Object(provides),
        }],
    })
}

/// Where this instance's data directory is: `data/<package>/<instance>`, because it is named.
fn data_directory(home: &Home, line: &Line) -> PathBuf {
    home.path().join("data").join("mariadb").join(line.instance)
}

/// `mix …` for a call that is expected to work, with the daemon's own log in the failure.
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

/// What `mix service status <id>` says.
fn status(home: &Home, line: &Line) -> Value {
    json(&home.mix(&["service", "status", line.service, "--json"]))
}

/// What this instance's server wrote about its own start.
fn server_log(home: &Home, line: &Line) -> String {
    let path = home
        .path()
        .join("logs")
        .join("services")
        .join(line.service)
        .join("mariadb.err");

    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// Every `service.first_run` job this home has run, for either instance.
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

/// A home with **both** MariaDBs installed and both services created, and the port each was given.
///
/// One index offering two versions of one package, which is this task's half of the install path:
/// `packages` is `UNIQUE (name, version)`, so two rows and two install paths are what a second
/// `package.install` of a different version is supposed to leave behind.
async fn created() -> (Home, harness::Daemon, MockRegistry, [u16; 2]) {
    let packing = if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    };

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-21T00:00:00Z", "packages": []
    }))
    .await;

    let entries = BOTH.map(|line| {
        let root = package(line);
        let packed = FakePackage::new(packing)
            .directory(&root)
            .build(&format!("mariadb-{}", line.version));
        let url = registry.publish_asset(&packed.path(), packed.bytes.clone());

        entry(line.version, &packed, &url, provides(&root))
    });

    registry.publish(&serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-21T00:00:00Z",
        "packages": entries,
    }));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());
    watch(&home);

    at("installing both versions, and creating an instance over each");
    let ports = BOTH.map(|line| {
        let installed = expect(
            &home,
            &["package", "install", "mariadb", line.version, "--json"],
        );
        assert_eq!(
            installed["state"],
            "succeeded",
            "{installed}\n{}",
            home.daemon_log()
        );

        let port = free_port();
        let created = expect(
            &home,
            &[
                "service",
                "create",
                line.service,
                line.version,
                "--port",
                &port.to_string(),
                "--json",
            ],
        );
        assert_eq!(
            created["service"]["id"],
            line.service,
            "{created}\n{}",
            home.daemon_log()
        );

        port
    });

    assert_ne!(ports[0], ports[1], "both instances were given one port");

    (home, daemon, registry, ports)
}

/// **The whole of T36, in the order a user meets it.**
///
/// One test rather than several, for `mariadb.rs`' reason and more so: every step here is the
/// previous one's precondition, and the expensive part — two real bootstraps — would be paid again
/// by every test that split off.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs two real MariaDBs — see the module note, and `mariadb-instances` in ci.yml"]
async fn two_instances_of_one_server_run_at_two_versions_without_knowing_about_each_other() {
    let (home, _daemon, _registry, _ports) = created().await;

    // --- both started, which is where each one bootstraps its own directory -----------------------
    at("starting both instances");
    for line in BOTH {
        let started = expect(&home, &["service", "start", line.service, "--json"]);
        assert_eq!(
            started["complete"],
            true,
            "{started}\n{}",
            home.daemon_log()
        );
    }

    let jobs = first_runs(&home);
    assert_eq!(
        jobs.len(),
        2,
        "two empty data directories ran {} first-run jobs\n{}",
        jobs.len(),
        home.daemon_log()
    );
    for job in &jobs {
        assert_eq!(job["state"], "succeeded", "{job}\n{}", home.daemon_log());
    }

    // --- and each directory says which version made it --------------------------------------------
    //
    // **The assertion this suite exists for.** Two markers, two versions, two servers — and each
    // marker is written by the ritual out of that row's own parent, so a second instance that had
    // quietly reused the first's package would be caught here and nowhere else.
    for line in BOTH {
        let data = data_directory(&home, line);
        let marker = std::fs::read_to_string(data.join(".mixengine-ready"))
            .unwrap_or_else(|error| panic!("{} has no marker: {error}", data.display()));

        assert_eq!(
            marker.trim(),
            line.version,
            "{} was bootstrapped by another version",
            data.display()
        );
        assert!(
            data.join("mysql").is_dir(),
            "the bootstrap left no system schema in {}",
            data.display()
        );
    }

    // --- both running, each proved by its own authenticated ping ----------------------------------
    //
    // `running` is the whole credential claim: the ready check is an authenticated
    // `mariadb-admin ping` with the password the daemon resolved out of the keyring, at
    // `mariadb@main/root` and at `mariadb@legacy/root`. Two instances sharing one entry, or one
    // overwriting the other's, cannot both reach this line.
    for line in BOTH {
        let up = status(&home, line);
        assert_eq!(
            up["state"],
            "running",
            "{} is not running\n{up}\n{}",
            line.service,
            home.daemon_log()
        );

        let said = server_log(&home, line);
        assert!(
            said.contains(line.version),
            "{} is not the version it was created at:\n{said}",
            line.service
        );
    }

    // --- stopping one leaves the other answering --------------------------------------------------
    //
    // The independence claim stated the only way it can be: not that the two are configured apart,
    // but that acting on one is not acting on the other.
    at("stopping one instance while the other keeps serving");
    let stopped = expect(&home, &["service", "stop", LEGACY.service, "--json"]);
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    let said = server_log(&home, &LEGACY);
    assert!(
        said.contains("Shutdown complete"),
        "the legacy server was terminated rather than asked to shut down:\n{said}"
    );

    let survivor = status(&home, &MAIN);
    assert_eq!(
        survivor["state"],
        "running",
        "stopping one instance stopped the other\n{survivor}\n{}",
        home.daemon_log()
    );

    // --- and starting it again does not bootstrap anything a second time ---------------------------
    at("starting the stopped instance again");
    let again = expect(&home, &["service", "start", LEGACY.service, "--json"]);
    assert_eq!(again["complete"], true, "{again}\n{}", home.daemon_log());

    assert_eq!(
        first_runs(&home).len(),
        2,
        "a second start bootstrapped a data directory again\n{}",
        home.daemon_log()
    );

    let again = status(&home, &LEGACY);
    assert_eq!(
        again["state"],
        "running",
        "the credential did not survive a restart\n{again}\n{}",
        home.daemon_log()
    );
}
