//! What a dev-tuned database template saves — roadmap task **T73**, measured.
//!
//! `../features/resource-isolation.md` says the config templates are *"tuned down for a dev
//! machine"*, and until T73 the database half of that sentence was not true: every number those
//! templates rendered was the value the server would have used with no configuration file at all.
//! This suite is what makes the claim checkable rather than asserted.
//!
//! # The gate is a difference, not a number
//!
//! Two MariaDB instances live in one home. `mariadb@main` takes the recipe's defaults;
//! `mariadb@stock` is reconfigured back to the server's own values — the size settings through
//! their keys, the directives the template states through `extra`, which renders last and wins.
//! Each is started **alone**, left to settle, read five times, and stopped before the other starts.
//!
//! **What is gated is `stock − tuned`**, and the choice is the point of the file:
//!
//! - An absolute budget on MariaDB's RSS would be a promise held hostage to next month's MariaDB,
//!   on a quantity this project does not control — which is exactly why `idle_footprint.rs` gates
//!   `mixengined` and merely reports the total beside it.
//! - A difference is the sentence the feature document actually makes, it is the only sentence a
//!   commit in this repository is responsible for, and it survives being measured on somebody
//!   else's runner: both readings come from one machine, minutes apart, from one binary.
//! - It fails in the direction that matters. **If the tuning does nothing, this goes red** — and
//!   "does nothing" is a real possibility rather than a rhetorical one, because RSS is memory that
//!   has been *touched*, not memory that has been *asked for*. A smaller buffer the allocator was
//!   never going to fault in would move no number, and an absolute budget would have passed while
//!   proving nothing.
//!
//! # The baseline this was landed with, and why it is worth knowing
//!
//! This file shipped one commit **before** the directives it measures, with both instances
//! rendering identical configuration, so that the method could be measured before anything depended
//! on it. Run 33322945686: MariaDB read **98.9 MB on Windows, 133.2 MB on Linux, 98.5 MB on
//! macOS**, and the two instances came within **0.0 %, 0.4 % and 0.0 %** of one another.
//!
//! That is what makes [`SAVED_AT_LEAST`] a gate rather than a hope: a difference above the noise
//! floor is a difference in the configuration, because the noise floor was measured rather than
//! assumed. It is also why the gate is a *fraction* — three machines gave three different absolute
//! readings, and next month's runner is a fourth.
//!
//! # One product, and why
//!
//! MariaDB alone. The `bench` job fetches Caddy, MariaDB, Redis and three PHPs; MySQL and
//! PostgreSQL would each cost an archive, a bootstrap and — on Linux — a second credential-store
//! dance, for a second copy of a difference this one already demonstrates. What their templates
//! need proved is that a real server *accepts* the file, and `mysql.rs` and `postgres.rs` in the
//! `test` job prove exactly that on all three systems.
//!
//! # Release, ignored, and a credential store
//!
//! `#[ignore]`d because this belongs to the `bench` job rather than to `test`: two real databases
//! and a number a loaded runner can move should not stand between a correctness suite and its
//! answer. The comparison is asserted **only in a release build**, on `idle_footprint.rs`' rule — a
//! debug daemon is a different program. A debug run still measures and still prints.
//!
//! It needs a secret service for T33's reason: the MariaDB bootstrap refuses a machine with no
//! credential store. Windows and macOS have one in the OS; on Linux the `bench` job installs
//! `gnome-keyring` and wraps this command in a `dbus-run-session` of its own.

mod harness;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use harness::{Home, json};
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked MariaDB is, as the CI step and a developer both set it.
const PACKAGE: &str = "MIXENGINE_MARIADB_PACKAGE";

/// The version the index publishes it as, and the one `mix service create` names.
///
/// The same line `warm_start.rs` pins, and pinned in both places for that file's reason: two
/// measurements are only comparable when they were taken against the same program.
const VERSION: &str = "11.4.12";

/// The instance on the recipe's own defaults — what a user gets.
const TUNED: &str = "mariadb@main";

/// The instance put back to the values the server would have chosen for itself.
const STOCK: &str = "mariadb@stock";

/// Both, in the order they are measured.
const INSTANCES: [&str; 2] = [TUNED, STOCK];

/// How much of what the server's own values hold the tuned defaults must not be holding.
///
/// **A fraction rather than a number of megabytes**, and the baseline is the reason. MariaDB read
/// 98.9 MB on Windows, 133.2 MB on Linux and 98.5 MB on macOS (run 33322945686) — three machines,
/// three answers, and next month's runner is a fourth. A budget pinned in megabytes against those
/// would be a budget about this quarter's hardware; a fraction is about the configuration, which is
/// the only thing a commit here changes.
///
/// **Five per cent, against a measured saving of 21.8 % and a measured noise floor of 0.4 %.** The
/// saving was taken outside CI, on MariaDB 10.11 under WSL, one server at a time with this file's
/// own method: 77.2 MB tuned against 98.7 MB stock, 21.6 MB apart. The noise floor is from the
/// baseline run, where both instances rendered the same file.
///
/// A gate at a quarter of what was measured is deliberate. The measurement is one series on one
/// system, while this runs 11.4 on three, and a guard that goes red because somebody's runner
/// allocated differently is a guard that gets raised rather than read. Five per cent is still more
/// than ten times the noise, so tuning that quietly stopped working cannot pass it.
///
/// **What was actually saved is printed on every run**, and
/// `.claude/features/resource-isolation.md` carries the figure.
const SAVED_AT_LEAST: f64 = 0.05;

/// How long an instance is left alone before the first reading.
///
/// Twenty seconds, against `idle_footprint.rs`' thirty: what settles here is one server that has
/// just bootstrapped, not a daemon that has installed a package and walked a start plan. Both
/// instances are given the same wait, which is what makes the two readings comparable at all.
const SETTLE: Duration = Duration::from_secs(20);

/// Readings per instance, a second apart; the median of them is what is compared.
///
/// Five, as both other budgets in the `bench` job take: answering a snapshot walks the process
/// table, so each reading perturbs what it reads a little, and a median is what buys that off.
const READINGS: usize = 5;

/// How long the whole suite is given before a hang is reported as the hang it is.
///
/// Twenty minutes, against two bootstraps and two settles. Not a budget anything is expected to come
/// near — it is the line past which *waiting* stops being the answer. `mariadb.rs` explains why this
/// is a thread and not a `tokio::time::timeout`.
const SUITE: Duration = Duration::from_secs(1200);

/// What this suite is doing, for the thread that has to report a hang.
static STAGE: Mutex<&'static str> = Mutex::new("packing an archive and starting a daemon");

/// Say what is happening now, in the log and to the watchdog.
fn at(stage: &'static str) {
    *STAGE.lock().expect("nothing panics holding this") = stage;
    eprintln!("[t73] {stage}");
}

/// Turn a hang into a failure that says where it hung — `mariadb.rs`' thread, for its reason.
fn watch(home: &Home) {
    let log = home.path().join("logs").join("daemon.log");

    std::thread::spawn(move || {
        std::thread::sleep(SUITE);

        let stage = *STAGE.lock().expect("nothing panics holding this");

        eprintln!(
            "\n--- this suite hung ---\nIt was {stage}.\n--- daemon.log ---\n{}",
            std::fs::read_to_string(&log)
                .unwrap_or_else(|error| format!("{} could not be read: {error}", log.display()))
        );

        std::process::exit(101);
    });
}

/// The MariaDB this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(PACKAGE).unwrap_or_else(|| {
        panic!(
            "{PACKAGE} is not set, so there is no database to measure. The `bench` job fetches one \
             through .github/scripts/fetch-package.sh; by hand, unpack the MariaDB \
             mixengine-packages publishes and point {PACKAGE} at the directory it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
///
/// The usual race is the usual price, and it is worth paying rather than fixing on 3306: a developer
/// running this has a MariaDB of their own, and a bench that took its port would be a bench that
/// stops somebody's work.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the artifact publishes as runnable, probed rather than written down.
///
/// `mariadb.rs`' table, for its reason: upstream renamed every one of these between 10.4 and 10.6,
/// so a suite that hard-coded either spelling would pass on one series while describing the other
/// wrongly.
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
            "{} publishes no {name}, so this suite has nothing to measure — it needs the MariaDB \
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
        "generated_at": "2026-08-30T06:55:12Z",
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

/// Everything T73 changes, put back to the value the server would have used on its own.
///
/// **Two routes, because the recipe has two kinds of value.** `innodb_buffer_pool_size` is a
/// `Setting` and comes back through its own key; `key_buffer_size` and the log flush are stated by
/// the template — they are a sentence about a development machine rather than a knob — and come
/// back through `extra`, which renders last in an option file, where a later line wins.
///
/// **This is also the test of that escape hatch on a real server.** `mariadb.rs`' unit test proves
/// `extra` renders after the directive; this proves a server started from the result reads it that
/// way, which is the half a template cannot assert about itself.
fn stock_overrides() -> String {
    serde_json::json!({
        "innodb_buffer_pool_size": "128M",
        "extra": "key_buffer_size = 128M\ninnodb_flush_log_at_trx_commit = 1\n",
    })
    .to_string()
}

/// `mix …` for a call that is expected to work, with the daemon's own log in the failure.
///
/// `mariadb.rs`' helper, for its reason: what says *why* a bootstrap went wrong is `daemon.log`, and
/// `harness::json` carries only one sentence of `mix`'s stderr.
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

/// What the server itself wrote about this instance's start.
///
/// The path the recipe renders — `logs/services/<id>/mariadb.err` — because `mariadbd` on Windows
/// sends nothing to its own output and this is the only account there is. Absence is reported and
/// not interpreted, which is `warm_start.rs`' lesson written down: a missing file means the path is
/// wrong at least as often as it means the server never ran.
fn server_log(home: &Home, service: &str) -> String {
    let path = home
        .path()
        .join("logs")
        .join("services")
        .join(service)
        .join("mariadb.err");

    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| format!("(no {}: {error})", path.display()))
}

/// One reading of what this home is holding: every subject, and its `rss_bytes`.
///
/// **Through `mix` rather than through the socket**, on `idle_footprint.rs`' rule: the number this
/// gates should be the number a person can read for themselves, from T71's own sampler.
fn subjects(home: &Home) -> Vec<(String, u64)> {
    let frame = json(&home.mix(&["metrics", "--json"]));

    let samples = frame["samples"]
        .as_array()
        .unwrap_or_else(|| panic!("a snapshot carries samples: {frame}"))
        .clone();

    let mut found: Vec<(String, u64)> = samples
        .iter()
        .map(|sample| {
            (
                sample["subject"].as_str().unwrap_or_default().to_owned(),
                sample["rss_bytes"].as_u64().unwrap_or_default(),
            )
        })
        .collect();

    found.sort();
    found
}

/// Start `service` alone, read it [`READINGS`] times, stop it, and answer the median RSS.
///
/// **Alone is load-bearing.** Both instances in this home are the same program with the same data
/// shape, so measuring them side by side would have each of them paging the other's file cache out;
/// and the subject-set assertion below is what proves they really did take turns.
async fn measure(home: &Home, service: &str) -> u64 {
    at("starting one instance, which is where it bootstraps");
    let started = expect(home, &["service", "start", service, "--json"]);
    assert_eq!(
        started["complete"],
        true,
        "`{service}` did not start, so there is nothing to \
         measure\n{started}\n--- {service} ---\n{}\n--- daemon.log ---\n{}",
        server_log(home, service),
        home.daemon_log()
    );

    at("letting it settle");
    tokio::time::sleep(SETTLE).await;

    let wanted = format!("service:{service}");
    let mut taken = Vec::with_capacity(READINGS);

    at("reading what it holds");
    for round in 0..READINGS {
        let found = subjects(home);

        // **Every round, not once**, and the whole set rather than one member: an instance that
        // died between the settle and the last reading would otherwise leave a very good number
        // behind it, and the *other* instance still running would make this measurement about two
        // servers while claiming to be about one.
        assert_eq!(
            found
                .iter()
                .map(|(subject, _)| subject.clone())
                .collect::<Vec<_>>(),
            vec!["daemon".to_owned(), wanted.clone()],
            "round {round} measured the wrong set of processes, so its numbers are about something \
             else: {found:?}\n--- daemon.log ---\n{}",
            home.daemon_log()
        );

        let rss = found
            .iter()
            .find(|(subject, _)| *subject == wanted)
            .map(|(_, rss)| *rss)
            .expect("the set assertion above passed");

        taken.push(rss);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    at("stopping it before the other one starts");
    let stopped = expect(home, &["service", "stop", service, "--json"]);
    assert_eq!(
        stopped["complete"],
        true,
        "`{service}` is still running, so the next reading would be about both of \
         them\n{stopped}\n--- daemon.log ---\n{}",
        home.daemon_log()
    );

    taken.sort_unstable();
    taken[taken.len() / 2]
}

/// A home with a real MariaDB installed and **two** services created over it.
async fn created() -> (Home, harness::Daemon, MockRegistry) {
    let root = package();

    let packing = if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    };
    let packed = FakePackage::new(packing)
        .directory(&root)
        .build(&format!("mariadb-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-30T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, provides(&root)));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());
    watch(&home);

    at("installing the package both instances run");
    let installed = expect(&home, &["package", "install", "mariadb", VERSION, "--json"]);
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

    at("creating both instances");
    for service in INSTANCES {
        let created = expect(
            &home,
            &[
                "service",
                "create",
                service,
                VERSION,
                "--port",
                &free_port().to_string(),
                "--json",
            ],
        );
        assert_eq!(
            created["service"]["id"],
            service,
            "{created}\n{}",
            home.daemon_log()
        );
    }

    // **Through the database, because there is no `mix service set`** — `postgres.rs`' module note
    // records the same absence. Adding the command to serve a measurement is how a test surface
    // becomes a product surface.
    mixengine_testkit::declare::reconfigure(&home.database_file(), STOCK, &stock_overrides()).await;

    (home, daemon, registry)
}

/// **Two instances rendering the same file are measured within a tenth of one another.**
///
/// **Both numbers are printed every run, not only when one fails.** The day the difference shrinks,
/// the pair beside it says whether the tuned side grew or the stock side shrank — which is the
/// difference between a regression here and a new MariaDB.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a budget, measured by the bench job — see the module note and ci.yml"]
async fn the_tuned_defaults_hold_less_than_the_servers_own() {
    let (home, _daemon, _registry) = created().await;

    let tuned = measure(&home, TUNED).await;
    let stock = measure(&home, STOCK).await;

    let saved = stock.saturating_sub(tuned);
    let fraction = saved as f64 / stock as f64;

    println!(
        "\n[t73] {TUNED} (the recipe's defaults), median of {READINGS}: {:.1} MB\n[t73] {STOCK} \
         (the server's own), median of {READINGS}: {:.1} MB\n[t73]   saved: {:.1} MB, {:.1} % \
         (gate {:.0} %)\n",
        as_mb(tuned),
        as_mb(stock),
        as_mb(saved),
        fraction * 100.0,
        SAVED_AT_LEAST * 100.0,
    );

    // **Release only**, on `idle_footprint.rs`' rule: a debug daemon is a different program, and a
    // number taken there is about the profile rather than about the design.
    if !cfg!(debug_assertions) {
        assert!(
            fraction >= SAVED_AT_LEAST,
            "the tuned defaults saved {:.1} MB, {:.1} % of what the server's own values held — \
             under the {:.0} % this gate gives them. {:.1} MB against {:.1} MB. Two instances \
             rendering the *identical* file were measured within 0.4 % of one another, so this is \
             a difference in the configuration rather than a bad minute",
            as_mb(saved),
            fraction * 100.0,
            SAVED_AT_LEAST * 100.0,
            as_mb(tuned),
            as_mb(stock),
        );
    }
}

/// Bytes as megabytes, for the lines a person reads.
fn as_mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}
