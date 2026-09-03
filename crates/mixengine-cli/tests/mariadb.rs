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
//!
//! # Why nothing in here ever reads that credential
//!
//! It did, once, and macOS is where that stopped. A keychain item carries an ACL naming the
//! application that created it, so the daemon reads its own credential without a word and **any
//! other process asking for it raises a dialog** — on a CI runner, one nobody can answer. Measured:
//! the whole suite bootstrapped, started and reached `running` in twenty-six seconds and then sat in
//! that read until the job's own timeout killed it, twenty-seven minutes later.
//!
//! It is not worth a `cfg`, because the assertion it was making is made better without it. **The
//! service reaching `running` is the proof**: its ready check is an authenticated `mariadb-admin
//! ping`, whose password the daemon resolves out of the keyring at spawn, so a store that held
//! nothing, or held the wrong thing, could not produce a running service. Everything else this
//! suite asks the server is a connection that must be **refused** — which needs no credential at
//! all, and says what a successful query never could: that there is no way in without one.

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

/// What the server itself wrote about this start: `<home>/logs/services/<id>/mariadb.err`.
///
/// **The server's own account, and the only one there is.** Windows `mariadbd` sends nothing to
/// standard output, so a supervisor reading the process's streams finds an empty file — which is
/// why the recipe points it at a log of its own, and why this is what both the version and the
/// clean shutdown are read out of.
fn server_log(home: &Home) -> String {
    let path = home
        .path()
        .join("logs")
        .join("services")
        .join(SERVICE)
        .join("mariadb.err");

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
    let client = root.join(format!("bin/mariadb{}", std::env::consts::EXE_SUFFIX));
    let client = if client.is_file() {
        client
    } else {
        root.join(format!("bin/mysql{}", std::env::consts::EXE_SUFFIX))
    };

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
    assert_eq!(
        created["service"]["id"],
        SERVICE,
        "{created}\n{}",
        home.daemon_log()
    );

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

    // **`running` is the assertion this whole suite exists for**, and it is worth saying why in the
    // place somebody will look for the query that used to be here. A TCP accept proves nothing — it
    // stays true for the whole of InnoDB's recovery, while the server refuses every statement. This
    // service's ready check is `mariadb-admin ping`, run by the daemon with the root password it
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
        refused.contains("'root'@"),
        "the server refused somebody other than root: {refused}"
    );

    // **The anonymous accounts are not asked about here, and the reason is worth writing down** —
    // an assertion was written, it passed, and it was measuring nothing. `mariadb-install-db`
    // creates `''@localhost` and `''@<hostname>`, and an anonymous row matches any user name that
    // is not otherwise defined, so the obvious test is to connect as somebody made up and expect a
    // refusal. Two things defeat it. The client will not send an empty user name — `--user=` falls
    // back to the login name, which is how the first version of this came to be refusing
    // `'haiqu'@'127.0.0.1'` and calling that proof. And this configuration says
    // `skip-name-resolve`, so a TCP connection's host is the literal `127.0.0.1`, which matches
    // neither `localhost` nor the hostname: those accounts could not let anybody in over this port
    // whether the `DELETE` ran or not.
    //
    // What would see them is a socket connection, which two of the three systems have. Reading
    // `mysql.global_priv` would too, and needs the credential this suite deliberately cannot read.
    // So the statement is covered where it is written instead — see the bootstrap test in
    // `recipes::mariadb` — and this suite claims only what a port can show.

    // --- a database and an account for it ---------------------------------------------------------
    //
    // **Nothing here reads a credential**, which is the whole reason the proof that the account's
    // password *works* is a step inside the daemon rather than an assertion here — roadmap task
    // T77a, design D13. See this file's module note for what reading one costs on macOS.
    at("creating a database and an account for it");
    let created = expect(
        &home,
        &["database", "create", SERVICE, "--name", "blog", "--json"],
    );
    assert_eq!(created["made"]["database"], "created", "{created}");
    assert_eq!(created["made"]["user"], "created", "{created}");
    assert_eq!(created["secret"], "mariadb@main/blog", "{created}");
    assert!(
        !created.to_string().contains("password"),
        "the answer carries the address of a credential and never the credential: {created}"
    );

    // **Asking twice changes nothing**, which is what makes a failed apply resumable rather than
    // restartable — and what `blueprint.apply` will lean on in T78.
    at("creating the same database a second time");
    let again = expect(
        &home,
        &["database", "create", SERVICE, "--name", "blog", "--json"],
    );
    assert_eq!(again["made"]["database"], "existing", "{again}");
    assert_eq!(again["made"]["user"], "existing", "{again}");

    // And the new account needs its password, the way root does: the same negative this suite makes
    // about root, aimed at the account the daemon just made.
    at("offering the server the new account with no password");
    let refused = without_a_password(&installed_at, port, "blog");
    assert!(
        refused.contains("'blog'@"),
        "the server refused somebody other than the new account: {refused}"
    );

    // A name no statement would take is refused before anything is created.
    at("asking for a database name that is not one");
    let bad = home.mix(&[
        "database",
        "create",
        SERVICE,
        "--name",
        "Blog; DROP",
        "--json",
    ]);
    assert!(
        !bad.status.success(),
        "a name outside the slug charset was accepted: {}",
        harness::stdout(&bad)
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

/// **The only test in the workspace in which a real credential reaches a real process through the
/// handoff** — roadmap task **T83**, its design's D2. A desktop entry in a data directory of this
/// test's own points at a script that records its first argument and *whether* the variable is
/// set — never its value, which is the module note's rule at one more address.
///
/// Linux only: it is the one system where a data directory can be named through the environment
/// of the daemon this test starts. The Windows and macOS lookups are asked for real, for an
/// application no machine has, in `database.rs`.
///
/// # Why this service is not `mariadb@main`
///
/// The two tests in this file run at once, in two homes, and **a Unix bootstrap keys two things on
/// the service's id alone**: the space-free view `/tmp/mixengine-init-<id>` that `mariadb-install-db`
/// is run through, and the keyring entry the root password lives in. Two homes bootstrapping one id
/// share both, and the view is the one that bites — the second ritual's first step is `rm -rf` on
/// it. Measured in WSL with the two steps a daemon runs: a second ritual starting 0.2 s or 0.5 s
/// after the first kills the first with `[ERROR] Aborting` as its last line, which is what CI
/// printed for the test above on the day this one joined it; at 1.5 s the first survives with one
/// file fewer in its data directory. A name of this test's own is what keeps the two apart; the
/// collision itself is written down in the roadmap beside T33.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real MariaDB — see the module note, and the `mariadb` step in ci.yml"]
async fn the_root_credential_reaches_the_client_through_its_environment_and_not_the_url() {
    use std::os::unix::fs::PermissionsExt as _;

    /// The service this test drives — see the note above for why it is not [`SERVICE`].
    const HANDOFF: &str = "mariadb@handoff";

    // The fake client: a script, a desktop entry naming it, and the file it reports into.
    let data = tempfile::tempdir().expect("a data dir");
    let record = data.path().join("received.txt");
    let script = data.path().join("fake-mixdb.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf 'url=%s\\n' \"$1\" > '{0}'\n\
             if [ -n \"$MIXENGINE_DB_PASSWORD\" ]; then echo 'password=present' >> '{0}'; \
             else echo 'password=absent' >> '{0}'; fi\n",
            record.display()
        ),
    )
    .expect("the script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("executable");
    let applications = data.path().join("applications");
    std::fs::create_dir_all(&applications).expect("applications");
    std::fs::write(
        applications.join("mixengine-fake.desktop"),
        format!(
            "[Desktop Entry]\nName=Fake\nExec={} %u\nType=Application\n",
            script.display()
        ),
    )
    .expect("the entry");

    // `created()`'s steps, with a daemon that reads its data directories from this test.
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
    let _daemon = home.start_daemon_reading_index_with_env(
        &registry.url(),
        registry.public_key(),
        &[("XDG_DATA_HOME", &data.path().display().to_string())],
    );
    watch(&home);

    at("installing the package");
    expect(&home, &["package", "install", "mariadb", VERSION, "--json"]);

    at("creating the service");
    expect(
        &home,
        &[
            "service",
            "create",
            HANDOFF,
            VERSION,
            "--port",
            &port.to_string(),
            "--json",
        ],
    );

    at("installing a desktop-app extension that names the fake client");
    let manifest = "schema = 1\n\n[extension]\nid = \"mixdb\"\nname = \"MixDB\"\n\
                    version = \"0.0.1\"\nkind = \"desktop-app\"\ndescription = \"fake\"\n\n\
                    [desktop-app]\nscheme = \"mixdb\"\n\n[desktop-app.detect]\n\
                    linux = \"mixengine-fake.desktop\"\n\n[permissions]\nnetwork = \"loopback\"\n";
    let extension = tempfile::tempdir().expect("an extension dir");
    std::fs::write(extension.path().join("extension.toml"), manifest).expect("the manifest");
    expect(
        &home,
        &[
            "extension",
            "install",
            "--path",
            &extension.path().display().to_string(),
            "--yes",
            "--json",
        ],
    );

    at("opening the database in the fake client, which starts the server and its first run");
    let opened = expect(&home, &["database", "open", HANDOFF, "--json"]);
    assert_eq!(opened["launched"]["launch"], "handed_on", "{opened}");
    assert_eq!(opened["secret"], "mariadb@handoff/root", "{opened}");
    assert_eq!(opened["client"]["state"], "installed", "{opened}");

    let received = std::fs::read_to_string(&record).expect("the script ran and wrote");
    assert!(
        received.contains("url=mixdb://connect?kind=mysql&host=127.0.0.1&port="),
        "{received}"
    );
    assert!(
        received.contains(&format!(
            "port={port}&user=root&label=mariadb%40handoff&password_env=MIXENGINE_DB_PASSWORD"
        )),
        "{received}"
    );
    assert!(received.contains("password=present"), "{received}");
    assert!(!received.contains("password=absent"), "{received}");

    at("stopping the service");
    expect(&home, &["service", "stop", HANDOFF, "--json"]);
}
