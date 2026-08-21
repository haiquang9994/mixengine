//! The MySQL recipe against a **real** MySQL — roadmap task **T34c**.
//!
//! Everything else about this recipe is provable in one process and is proved there: the three
//! bootstrap routes, the template, the spec, the credential that is named rather than carried. None
//! of that says the thing the task is about, which is that *a data directory MixEngine bootstrapped
//! becomes a database that answers a query as the root it generated a password for* — and here that
//! claim rests on two mechanisms MariaDB never needed: a server started with `--skip-networking` to
//! set the password, and a file the daemon writes and removes around that one step, because MySQL
//! removed `--bootstrap` at 5.7.6.
//!
//! **It is `#[ignore]`d rather than skipped**, for `mariadb.rs`' reason: a test that quietly returns
//! when it finds no MySQL is a green suite that proved nothing on the day the download broke.
//!
//! **This suite needs a credential store**, as the MariaDB one does, and reads no credential for the
//! same measured reason — a macOS keychain item belongs to the process that created it, and any
//! other process asking raises a dialog nobody on a CI runner can answer. What proves the password
//! is the service reaching `running`: its ready check is an authenticated `mysqladmin ping`, whose
//! password the daemon resolves out of the keyring at spawn. Everything else this suite asks the
//! server is a connection that must be **refused**.
//!
//! **The version is 8.4**, the line `.claude/features/services.md` names. The other two routes — 5.6
//! on Unix through its Perl installer, and 5.6 on Windows copying the `data/` directory upstream
//! ships — are covered by `mixengine-packages`' own smoke test on every published cell, and by the
//! route table in `recipes::mysql`. What is here is the one a user gets by default.

mod harness;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use harness::{Home, json};
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
    eprintln!("[mysql] {stage}");
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

/// Where an unpacked MySQL is, as the CI step and a developer both set it.
///
/// The directory the archive unpacks to — `bin/mysqld` inside it — which is also what a `packages`
/// row's `install_path` is.
const PACKAGE: &str = "MIXENGINE_MYSQL_PACKAGE";

/// The version the index publishes it as, and the one `mix service create` names.
const VERSION: &str = "8.4.10";

/// The service this suite drives. **An `@`**, which is the instancing rule seen from a recipe that
/// has it: a home may hold two databases, so every one of them is named.
const SERVICE: &str = "mysql@main";

/// The MySQL this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no MySQL to judge this recipe against. The \
             `mysql` step in .github/workflows/ci.yml fetches one; by hand, unpack any MySQL \
             8.4 from mixengine-packages' releases and point {PACKAGE} at the directory it \
             unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and it is worth paying here rather than fixing on 3306: a
/// developer running this suite very likely has a MySQL of their own, and a test that took its
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
/// **Probed rather than written down**, for the reason MariaDB's equivalent is: the layout belongs
/// to whoever published the archive. Three commands and no installer — `mysql_install_db` is 5.6 and
/// 5.7 only, and the line this suite runs bootstraps itself.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for name in ["mysqld", "mysql", "mysqladmin"] {
        let relative = format!("bin/{name}{}", std::env::consts::EXE_SUFFIX);

        // Named here rather than left to the recipe: `ServiceProvidesNothing` arriving from three
        // layers down at `service start` says the same thing much later and much less clearly.
        assert!(
            root.join(&relative).is_file(),
            "{} publishes no {name}, so this suite has nothing to drive — it needs the MySQL \
             mixengine-packages builds, not a system one",
            root.display()
        );

        found.insert(name.to_owned(), Value::String(relative));
    }

    found
}

/// An index offering exactly this MySQL, for this machine.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-19T06:55:12Z",
        "packages": [{
            "kind": "mysql",
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
    home.path().join("data").join("mysql").join("main")
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

/// What `mix service status mysql@main` says.
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

/// What the server itself wrote about this start: `<home>/logs/services/<id>/mysql.err`.
///
/// **The server's own account, and the only one there is.** Windows `mysqld` sends nothing to
/// standard output, so a supervisor reading the process's streams finds an empty file — which is
/// why the recipe points it at a log of its own, and why this is what both the version and the
/// clean shutdown are read out of.
fn server_log(home: &Home) -> String {
    let path = home
        .path()
        .join("logs")
        .join("services")
        .join(SERVICE)
        .join("mysql.err");

    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", path.display()))
}

/// Try to log in as `user` with no password, and hand back what the client said.
///
/// The shipped client rather than a Rust driver: the workspace has no MySQL protocol implementation
/// and has no reason to grow one for a test, and the client is in the archive this suite installed.
///
/// **Every connection this suite makes is one that must be refused**, and `--password=` is how: an
/// empty password rather than none at all, so the client sends credentials and is turned away
/// instead of falling back to a socket or prompting for one. See the module note for why this is
/// the shape of the assertion.
fn without_a_password(root: &Path, port: u16, user: &str) -> String {
    let client = root.join(format!("bin/mysql{}", std::env::consts::EXE_SUFFIX));

    let refused = Command::new(client)
        .args([
            "--protocol=TCP",
            "--host=127.0.0.1",
            &format!("--port={port}"),
            &format!("--user={user}"),
            "--password=",
            "--batch",
            "--skip-column-names",
            "-e",
            "SELECT 1;",
        ])
        .output()
        .expect("the client in the archive can be run");

    // **A refusal, and not merely a failure.** Every one of these assertions is a negative, so a
    // client that could not run, could not resolve the host, or was handed an option this series
    // does not know would satisfy them all while proving nothing. The server's own `Access denied`
    // — with the account it decided the connection was for — is the only answer that means what
    // this suite needs it to mean, so it is what is returned and what the caller matches on.
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );

    assert!(
        !refused.status.success(),
        "the server let a passwordless connection in as `{user}`: {said}"
    );
    assert!(
        said.contains("Access denied"),
        "the client failed before the server could refuse it, so this proves nothing about          `{user}`: {said}"
    );

    said
}

/// A home with a real MySQL installed in it, a service created over it, and the port it will use.
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
        .build(&format!("mysql-{VERSION}"));

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
    let installed = expect(&home, &["package", "install", "mysql", VERSION, "--json"]);
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
    assert_eq!(
        created["service"]["id"],
        SERVICE,
        "{created}\n{}",
        home.daemon_log()
    );

    // The install path this home unpacked into, which is where the client comes from.
    let installed_at = home.path().join("packages").join("mysql").join(VERSION);

    (home, daemon, registry, installed_at, port)
}

/// **The whole of T34c, in the order a user meets it.**
///
/// One test rather than eight, deliberately: each step is the previous one's precondition, and eight
/// tests would be eight real bootstraps performed to re-reach the state this one is already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real MySQL — see the module note, and the `mysql` step in ci.yml"]
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

    // **`running` is the assertion this whole suite exists for**, and it is worth saying why in the
    // place somebody will look for the query that used to be here. A TCP accept proves nothing — it
    // stays true for the whole of InnoDB's recovery, while the server refuses every statement. This
    // service's ready check is `mysqladmin ping`, run by the daemon with the root password it
    // resolved out of the keyring at spawn. A store holding nothing, a store holding the wrong
    // value, a bootstrap that never set the password: none of them reach the line above.
    //
    // And the server says which build answered, in its own log.
    let said = server_log(&home);
    assert!(
        said.contains(VERSION),
        "the server that came up is not the one installed:\n{said}"
    );

    // **Root without a password is refused**, which is the other half of the same claim: the ping
    // above proves a password works, and this proves it is a password rather than a formality.
    at("offering the server a root login with no password");
    let refused = without_a_password(&installed_at, port, "root");
    assert!(
        refused.contains("'root'@'localhost'"),
        "the server refused an account other than the one `--initialize-insecure` creates, which \
         means the name lookup this template deliberately leaves on did not happen: {refused}"
    );

    // **There are no anonymous accounts to ask about on this route, and that is the route's
    // whole shape.** `--initialize-insecure` creates `root@localhost` and nothing else: no
    // anonymous rows and no `test` database, which is why the modern branch of this recipe has
    // no clean-up statements where MariaDB's bootstrap has three. The 5.6 branch starts from a
    // directory an installer or upstream's own zip built and does have them, and they are
    // asserted where they are written: see the bootstrap test in `recipes::mysql`.
    //
    // **And the lookup that makes that account reachable over TCP is left on**, which is the
    // measured difference from MariaDB and is what the refusal above quietly proves: the server
    // named the account it turned away, so it resolved 127.0.0.1 to `localhost` and found one.
    // With `skip-name-resolve` in this template it would have found nothing — and the
    // authenticated ping that made this service `running` would not have worked either.

    // --- stopped, cleanly ------------------------------------------------------------------------
    at("stopping the service");
    let stopped = expect(&home, &["service", "stop", SERVICE, "--json"]);
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    // **Proof, rather than a belief about an exit code.** A `mysqld` that was terminated exits
    // zero and leaves a dirty buffer pool; only the server's own log says the shutdown finished.
    let said = server_log(&home);
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

    // The credential survived the restart, said the way the first start said it: a service that
    // reaches `running` has answered an authenticated ping, and this one was never bootstrapped a
    // second time to re-write the password it answered with.
    let again = status(&home);
    assert_eq!(
        again["state"],
        "running",
        "the credential did not survive a restart\n{again}\n{}",
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
