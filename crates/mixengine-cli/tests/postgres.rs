//! The PostgreSQL recipe against a **real** PostgreSQL — roadmap task **T34**.
//!
//! Everything else about this recipe is provable in one process and is proved there: three files
//! render with every path quoted, no line of `pg_hba.conf` says `trust`, the spec builds, the file
//! and the readiness check name one port, the ritual sets a password without writing one to disk.
//! None of that says the thing the task is about, which is that **a cluster MixEngine bootstrapped
//! becomes a server that answers an authenticated query with the password MixEngine generated**.
//!
//! **It is `#[ignore]`d rather than skipped**: a test that quietly returns when it finds no
//! PostgreSQL is a green suite that proved nothing on the day the download broke.
//!
//! **This suite needs a credential store**, as `mariadb.rs` does and for the same reason — the
//! superuser password has exactly one home and the ritual refuses a machine with none. On Linux
//! `.github/scripts/test-no-network.sh` starts a `gnome-keyring` on a session bus of its own, which
//! is why the Linux leg runs this from inside that script rather than beside it.
//!
//! **And on Windows it needs T34a.** `postgres` refuses a token holding an enabled
//! `BUILTIN\Administrators`, and this repository's Windows runner holds one deliberately (T2b) — so
//! before T34a this suite could not have run there at all. That it runs is the assertion T34a buys.
//!
//! Nothing here ever reads the credential, for `mariadb.rs`' measured reason: a macOS keychain item
//! carries an ACL naming its creator, and any other process asking raises a dialog nobody on a
//! runner can answer.
//!
//! # The reload is not exercised here, and that is a gap with an owner
//!
//! `pg_ctl reload` is in the spec and asserted where it is written — see
//! `recipes::postgres::postgres_reloads_the_same_way_on_every_system`. What cannot be driven from
//! out here is *asking* for one: there is no `service.reload` in
//! [`rpc`](mixengine_proto::rpc::method) and no `mix service set`, so a running server has no way to
//! be handed a changed file and told to re-read it. Nothing in T34 could have added one without
//! adding an RPC and a subcommand this task did not ask for. The claim this suite makes is
//! therefore the bootstrap and the credential, and the reload waits for the task that gives a
//! service a way to be reconfigured.

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
/// Twelve minutes, `mariadb.rs`' number: it is not a budget anything is expected to come near, it
/// is the line past which *waiting* stops being the answer.
const BUDGET: Duration = Duration::from_secs(720);

/// What this suite is doing, for the thread that has to report a hang.
static STAGE: Mutex<&'static str> = Mutex::new("packing the archive and starting a daemon");

/// Say what is happening now, in the log and to the watchdog.
fn at(stage: &'static str) {
    *STAGE.lock().expect("nothing panics holding this") = stage;
    eprintln!("[postgres] {stage}");
}

/// Turn a hang into a failure that says where it hung.
///
/// A thread rather than a `tokio::time::timeout`, for the reason `mariadb.rs` records: the calls
/// this suite makes are blocking ones — a client process, a keyring round trip — and an async
/// deadline around a blocked thread never fires.
fn watch(home: &Home) {
    let log = home.path().join("logs").join("daemon.log");

    std::thread::spawn(move || {
        std::thread::sleep(BUDGET);

        let stage = *STAGE.lock().expect("nothing panics holding this");

        eprintln!(
            "\n--- this suite hung ---\nIt was {stage}, and it had been for less than {BUDGET:?}.\
             \n--- daemon.log ---\n{}",
            std::fs::read_to_string(&log)
                .unwrap_or_else(|error| format!("{} could not be read: {error}", log.display()))
        );

        std::process::exit(101);
    });
}

/// Where an unpacked PostgreSQL is, as the CI step and a developer both set it.
const PACKAGE: &str = "MIXENGINE_POSTGRES_PACKAGE";

/// The version the index publishes it as.
const VERSION: &str = "18.6";

/// The service this suite drives. **An `@`**, because a home may hold two clusters.
const SERVICE: &str = "postgres@main";

/// The PostgreSQL this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no PostgreSQL to judge this recipe against. The \
             `postgres` step in .github/workflows/ci.yml fetches one; by hand, unpack any \
             PostgreSQL from mixengine-packages' releases and point {PACKAGE} at the directory it \
             unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and it is worth paying here rather than fixing on 5432: a
/// developer running this suite very likely has a PostgreSQL of their own.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes, as an index entry says it.
///
/// One spelling per command on every route — the Debian cell buys that with a symlink, which
/// `mixengine-packages`' own packaging explains — so this asserts the layout rather than searching
/// it. The five required ones only: the artifact publishes fifteen, and a recipe may rely on these.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for name in ["postgres", "initdb", "pg_ctl", "psql", "pg_isready"] {
        let relative = format!("bin/{name}{}", std::env::consts::EXE_SUFFIX);

        // Named here rather than left to the recipe: `ServiceProvidesNothing` arriving from three
        // layers down at `service start` says the same thing much later and much less clearly.
        assert!(
            root.join(&relative).is_file(),
            "{} publishes no {name}, so this suite has nothing to drive — it needs the PostgreSQL \
             mixengine-packages builds, not a system one",
            root.display()
        );

        found.insert(name.to_owned(), Value::String(relative));
    }

    found
}

/// An index offering exactly this PostgreSQL, for this machine.
fn index(packed: &Packed, url: &str, provides: serde_json::Map<String, Value>) -> Value {
    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-20T06:55:12Z",
        "packages": [{
            "kind": "postgres",
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
    home.path().join("data").join("postgres").join("main")
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

/// What `mix service status postgres@main` says.
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

/// What the server itself wrote about this start.
///
/// `logging_collector = off` and `log_destination = 'stderr'`, so this is the supervisor's own
/// capture rather than a file inside the data directory — which is the whole reason the recipe
/// states both.
fn server_log(home: &Home) -> String {
    let directory = home.path().join("logs").join("services").join(SERVICE);

    let mut said = String::new();

    for entry in std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", directory.display()))
    {
        let path = entry.expect("a directory entry").path();

        if path.is_file() {
            said.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }

    said
}

/// Try to connect as `user` with no password, and hand back what the client said.
///
/// **Every connection this suite makes is one that must be refused.** `PGPASSWORD=` — empty rather
/// than unset — so `psql` sends credentials and is turned away instead of prompting; `--no-password`
/// so that a client which decides to prompt anyway fails instead of hanging. Measured: without it,
/// `psql` waits at `Password:` for ever, and a suite that hung there would be killed by the job's
/// own timeout half an hour later.
fn without_a_password(root: &Path, port: u16, user: &str, database: &str) -> String {
    let psql = root.join(format!("bin/psql{}", std::env::consts::EXE_SUFFIX));

    let refused = Command::new(psql)
        .env("PGPASSWORD", "")
        .args([
            "--host=127.0.0.1",
            &format!("--port={port}"),
            &format!("--username={user}"),
            &format!("--dbname={database}"),
            "--no-password",
            "-tAc",
            "SELECT 1",
        ])
        .output()
        .expect("the client in the archive can be run");

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );

    // **A refusal, and not merely a failure.** Every assertion here is a negative, so a client that
    // could not run at all would satisfy them while proving nothing. The server's own
    // authentication complaint is the only answer that means what this needs it to mean.
    assert!(
        !refused.status.success(),
        "the server let a passwordless connection in as `{user}`: {said}"
    );
    assert!(
        said.contains("authentication failed") || said.contains("no password supplied"),
        "the client failed before the server could refuse it, so this proves nothing: {said}"
    );

    said
}

/// A home with a real PostgreSQL installed in it, a service created over it, and the port it uses.
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
        .build(&format!("postgres-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-20T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());
    watch(&home);

    at("installing the package");
    let installed = expect(
        &home,
        &["package", "install", "postgres", VERSION, "--json"],
    );
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
    let installed_at = home.path().join("packages").join("postgres").join(VERSION);

    (home, daemon, registry, installed_at, port)
}

/// **The whole of T34 that needs a server, in the order a user meets it.**
///
/// One test rather than six, deliberately: each step is the previous one's precondition, and six
/// tests would be six real bootstraps performed to re-reach the state this one is already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PostgreSQL — see the module note, and the `postgres` step in ci.yml"]
async fn a_cluster_is_bootstrapped_started_queried_stopped_and_not_bootstrapped_twice() {
    let (home, _daemon, _registry, installed_at, port) = created().await;
    let data = data_directory(&home);

    // --- started, which is where the bootstrap happens -------------------------------------------
    //
    // Nothing here asked for a first run. The start did, because the data directory is empty.
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

    // --- and left a cluster that says who made it -------------------------------------------------
    let marker = std::fs::read_to_string(data.join(".mixengine-ready"))
        .expect("a finished ritual leaves its marker");
    assert_eq!(marker.trim(), VERSION, "the marker names another version");
    assert!(
        data.join("base").is_dir(),
        "initdb left no cluster in {}",
        data.display()
    );

    // **The cluster's own configuration is untouched and unread**, which is the claim that keeps
    // `etc/` disposable: `initdb` wrote a `pg_hba.conf` in there, the ritual asked it for `reject`
    // on every line, and the server never looked at it — `hba_file` names the generated one.
    let inherited =
        std::fs::read_to_string(data.join("pg_hba.conf")).expect("initdb writes one of its own");
    assert!(
        inherited
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .any(|line| line.split_whitespace().any(|word| word == "reject")),
        "initdb stopped writing the file this test is about; the claim above needs rewording:\n\
         {inherited}"
    );

    // --- running, and proved so by an authenticated query rather than by an accept -----------------
    //
    // **`running` is the assertion this whole suite exists for.** `pg_isready` would answer for a
    // cluster whose password never got set; this service's ready check is `psql -tAc "SELECT 1"`
    // run by the daemon as `postgres` with the password it resolved out of the keyring at spawn. A
    // store holding nothing, a store holding the wrong value, a `--single` step that never ran:
    // none of them reach this line. Which matters twice over here, because `postgres --single`
    // **exits zero even when the statement it was fed failed** — measured — so nothing else in this
    // chain could have caught a password that was never set.
    let up = status(&home);
    assert_eq!(up["state"], "running", "{up}\n{}", home.daemon_log());

    let said = server_log(&home);
    assert!(
        said.contains("database system is ready to accept connections"),
        "the server never announced itself:\n{said}"
    );

    // **And the superuser is refused without it**, which is the other half of the same claim: the
    // query above proves a password works, and this proves it is a password rather than a
    // formality — and that `pg_hba.conf`'s `scram-sha-256` is what the server is reading.
    at("offering the server a superuser login with no password");
    without_a_password(&installed_at, port, "postgres", "postgres");

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
    assert_eq!(created["secret"], "postgres@main/blog", "{created}");
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

    // And the new role needs its password, the way the superuser does — the same negative this suite
    // already makes, aimed at the role the daemon just made. That it reaches `blog` at all is also
    // the only thing here that shows `pg_hba.conf` lets a non-superuser in.
    at("offering the server the new role with no password");
    without_a_password(&installed_at, port, "blog", "blog");

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

    // --- stopped, cleanly --------------------------------------------------------------------------
    at("stopping the service");
    let stopped = expect(&home, &["service", "stop", SERVICE, "--json"]);
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    // **Proof, rather than a belief about an exit code.** A terminated postmaster leaves an unclean
    // shutdown and the next start replays the write-ahead log; only the server's own log says the
    // shutdown finished.
    let said = server_log(&home);
    assert!(
        said.contains("database system is shut down"),
        "the server was killed rather than asked to shut down:\n{said}"
    );

    // --- started a second time, and not bootstrapped again ------------------------------------------
    at("starting it a second time, which must not bootstrap again");
    let again = expect(&home, &["service", "start", SERVICE, "--json"]);
    assert_eq!(again["complete"], true, "{again}\n{}", home.daemon_log());

    assert_eq!(
        first_runs(&home).len(),
        1,
        "a second start bootstrapped the cluster again\n{}",
        home.daemon_log()
    );

    // The credential survived the restart, said the way the first start said it: a service that
    // reaches `running` has answered an authenticated query, and this one was never bootstrapped a
    // second time to re-write the password it answered with.
    let again = status(&home);
    assert_eq!(
        again["state"],
        "running",
        "the credential did not survive a restart\n{again}\n{}",
        home.daemon_log()
    );

    // --- and a cluster that is not ours is refused, not cleaned --------------------------------------
    let stopped = expect(&home, &["service", "stop", SERVICE, "--json"]);
    assert_eq!(stopped["complete"], true, "{stopped}");

    std::fs::remove_file(data.join(".mixengine-ready")).expect("the marker is there");
    // Beside the directory rather than inside it — see `first_run::STARTED_MARKER`, and `initdb`,
    // which refuses a data directory with anything at all in it.
    let _ = std::fs::remove_file(data.with_file_name("main.mixengine-init-started"));

    at("starting over a cluster MixEngine did not create");
    let refused = home.mix(&["service", "start", SERVICE, "--json"]);
    let said = harness::stdout(&refused);
    assert!(
        !refused.status.success(),
        "a cluster MixEngine did not create was bootstrapped over\n{said}\n{}",
        home.daemon_log()
    );
    assert!(
        said.contains("was not created by MixEngine"),
        "the start failed for some other reason: {said}"
    );
    assert!(
        data.join("base").is_dir(),
        "the refusal removed somebody's database"
    );
}
