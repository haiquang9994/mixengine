//! The generated ini set against a **real** PHP — roadmap task **T28**.
//!
//! **The terminal and the pool are why this suite exists.** Everything else is provable in one
//! process and is proved there: the merge, the ordering, the two `zend_extension` names, the sweep.
//! What cannot be proved there is that the set actually reaches PHP — through the shim on a terminal
//! *and* through the pool in a browser — and on Windows that is where it fails first, because
//! `curl`, `mbstring` and `intl` are shared modules there that only an ini switches on.
//!
//! It also settles the two things the design asserted rather than measured: that php-fpm's `SIGUSR2`
//! picks up a *newly enabled* extension, and that `zend_extension = xdebug` spelled as a bare name is
//! a spelling this PHP accepts. Both fail quietly — a `zend_extension` PHP cannot load is a startup
//! warning, not a refusal to start — which is why every assertion below compares **loaded sets**
//! rather than exit codes.
//!
//! **`#[ignore]`d rather than skipped**, for `php_fpm.rs`' reason: a test that quietly returns when
//! it finds no PHP is a green suite that proved nothing on the day the download broke.

mod harness;

use std::collections::BTreeSet;
#[cfg(windows)]
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use harness::{Home, json};
use mixengine_testkit::fastcgi::Pool;
use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

/// Where an unpacked PHP is, as the CI step and a developer both set it.
const RUNTIME: &str = "MIXENGINE_PHP_RUNTIME";

/// The version the index publishes it as, and the half after the `@` in the pool's id.
const VERSION: &str = "8.3.33";

/// How long the pool is given to be serving again after its ini set moved under it.
const EVENTUALLY: Duration = Duration::from_secs(30);

/// The service this suite drives, which nobody in it creates.
fn pool() -> String {
    format!("php-fpm@{VERSION}")
}

/// The PHP this suite is about, or the reason there is none.
fn package() -> PathBuf {
    let directory = std::env::var_os(RUNTIME).unwrap_or_else(|| {
        panic!(
            "{RUNTIME} is not set, so there is no PHP to judge this ini set against. The `php` \
             step in .github/workflows/ci.yml fetches one; by hand, unpack any PHP 8.3 from \
             mixengine-packages' releases and point {RUNTIME} at the directory it unpacked to."
        )
    });

    PathBuf::from(directory)
}

/// A port nothing is listening on, by listening on it and then not.
#[cfg(windows)]
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("a loopback port")
        .local_addr()
        .expect("the port it was given")
        .port()
}

/// What the archive says about itself.
///
/// **Read rather than probed.** Every archive `mixengine-packages` publishes carries a
/// `mixengine-artifact.json` holding the same facts the index does — `provides`, `extension_dir`,
/// and the `static`/`shared`/`enabled` split — because the publishing pipeline writes both from one
/// source. A suite that worked the layout out for itself would be describing whichever build was
/// current when it was written, and would quietly stop matching the index the daemon really reads.
fn artifact(root: &Path) -> Value {
    let file = root.join("mixengine-artifact.json");

    let text = std::fs::read_to_string(&file).unwrap_or_else(|error| {
        panic!(
            "{} carries no mixengine-artifact.json ({error}) — this suite needs the PHP \
             mixengine-packages builds, not a system one",
            root.display()
        )
    });

    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is not readable: {error}", file.display()))
}

/// The SAPI this system's pool runs, named here rather than left to the recipe.
///
/// `ServiceProvidesNothing` arriving from three layers down at `service start` says the same thing
/// much later and much less clearly.
fn insist_on_a_sapi(artifact: &Value, root: &Path) {
    let sapi = if cfg!(windows) { "php-cgi" } else { "php-fpm" };

    assert!(
        artifact["provides"].get(sapi).is_some(),
        "{} publishes no {sapi}, so there is nothing here for a pool to run",
        root.display()
    );
}

/// An index offering exactly this PHP, out of the facts the archive itself carries.
///
/// The one thing not copied straight through is `enabled`: xdebug is taken out of it if the build
/// ships it switched on, because a profiler that is already loaded is not a pair this suite can turn
/// round. Every build seen so far ships it off, which is what an index does with one.
fn index(packed: &Packed, url: &str, artifact: &Value) -> Value {
    let mut extensions = artifact["extensions"].clone();

    if let Some(enabled) = extensions["enabled"].as_array() {
        let without: Vec<Value> = enabled
            .iter()
            .filter(|name| *name != "xdebug")
            .cloned()
            .collect();
        extensions["enabled"] = Value::Array(without);
    }

    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-20T06:55:12Z",
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
                "provides": artifact["provides"].clone(),
                "extension_dir": artifact["extension_dir"].clone(),
                "extensions": extensions,
            }],
        }],
    })
}

/// The names that are a **SAPI** rather than an extension.
///
/// `get_loaded_extensions()` reports the server API PHP is running under as though it were a module
/// — `cgi-fcgi` through the pool on Windows, `fpm-fcgi` through php-fpm — and `php -m` on a terminal
/// reports the CLI's. That difference is the two SAPIs being two SAPIs, which is the one thing this
/// suite is *not* asking about: what it compares is the ini set both of them read.
const SAPI: [&str; 5] = [
    "cli",
    "cli-server",
    "cgi-fcgi",
    "fpm-fcgi",
    "apache2handler",
];

/// What `php -m` says **through the shim in `<home>/bin`**, as a set.
///
/// The shim and not the runtime's own binary: what is being proved is that the resolution puts
/// `PHP_INI_SCAN_DIR` in front of the program, and running `runtimes/php/…/bin/php` directly would
/// prove the opposite of what this suite is for.
fn through_the_terminal(home: &Home) -> BTreeSet<String> {
    let php = home
        .path()
        .join("bin")
        .join(format!("php{}", std::env::consts::EXE_SUFFIX));

    // `MIXENGINE_HOME`, because a shim with none resolves against *this machine's* install rather
    // than the home this test made — the one input a shim has beside its own name.
    let ran = std::process::Command::new(&php)
        .arg("-m")
        .env("MIXENGINE_HOME", home.path())
        .output()
        .unwrap_or_else(|error| panic!("{} did not run: {error}", php.display()));

    assert!(
        ran.status.success(),
        "`php -m` failed through the shim: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    String::from_utf8_lossy(&ran.stdout)
        .lines()
        .map(str::trim)
        // `php -m` prints `[PHP Modules]` and `[Zend Modules]` as headings.
        .filter(|line| !line.is_empty() && !line.starts_with('['))
        .map(str::to_lowercase)
        .filter(|name| !SAPI.contains(&name.as_str()))
        .collect()
}

/// What `get_loaded_extensions()` says **through the pool**, as a set.
fn through_the_pool(listen: &Pool, script: &Path) -> BTreeSet<String> {
    let answered = listen
        .get(script)
        .expect("the pool answered a FastCGI request");

    answered
        .body
        .rsplit('\n')
        .find(|line| line.contains(','))
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_lowercase)
        .filter(|name| !SAPI.contains(&name.as_str()))
        .collect()
}

/// The assertion this suite exists to make, phrased so a failure names the extension and the side.
fn agree(terminal: &BTreeSet<String>, pool: &BTreeSet<String>, when: &str) {
    let only_terminal: Vec<&String> = terminal.difference(pool).collect();
    let only_pool: Vec<&String> = pool.difference(terminal).collect();

    assert!(
        only_terminal.is_empty() && only_pool.is_empty(),
        "the terminal and the pool disagree {when}\n  only `php -m`: {only_terminal:?}\n  \
         only the pool: {only_pool:?}"
    );
    assert!(
        !terminal.is_empty(),
        "neither side loaded anything at all {when}"
    );
}

/// A home with a real PHP installed in it, a daemon over it, and the endpoint its pool listens on.
async fn installed() -> (Home, harness::Daemon, MockRegistry, Pool) {
    let root = package();
    let artifact = artifact(&root);
    insist_on_a_sapi(&artifact, &root);

    assert!(
        artifact["extensions"]["shared"]
            .as_array()
            .is_some_and(|shared| shared.iter().any(|name| name == "xdebug")),
        "this PHP ships no xdebug, and xdebug shipped-and-off is the pair the whole task turns on"
    );

    let packing = if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    };
    let packed = FakePackage::new(packing)
        .directory(&root)
        .build(&format!("php-{VERSION}"));

    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-20T06:55:12Z", "packages": []
    }))
    .await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&index(&packed, &url, &artifact));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed = json(&home.mix(&["runtime", "install", "php", VERSION, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

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

/// Wait until the pool answers with a loaded set that satisfies `wanted`.
///
/// A poll and not a sleep, for `php_fpm.rs`' reason: what is being waited on is a master cycling its
/// workers on a runner that may be compiling something else at the same time.
fn eventually(
    listen: &Pool,
    script: &Path,
    wanted: impl Fn(&BTreeSet<String>) -> bool,
) -> BTreeSet<String> {
    let deadline = Instant::now() + EVENTUALLY;

    loop {
        let loaded = through_the_pool(listen, script);

        if wanted(&loaded) {
            return loaded;
        }

        assert!(
            Instant::now() < deadline,
            "the pool never came round to the set it was told about: {loaded:?}"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// **The whole of T28, in the order a user meets it.**
///
/// One test rather than seven, for `php_fpm.rs`' reason: each step is the previous one's
/// precondition, and seven tests would be seven real PHP installs performed to re-reach the state
/// this one is already in.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real PHP — see the module note, and the `php` step in ci.yml"]
async fn one_ini_set_reaches_the_terminal_and_the_pool_and_moves_when_it_is_told_to() {
    let (home, _daemon, _registry, listen) = installed().await;
    let pool = pool();

    // --- the listing says what the build ships, and who decided ---------------------------------
    let listed = json(&home.mix(&["runtime", "ext", "list", "--php", VERSION, "--json"]));
    let of = |name: &str| {
        listed["extensions"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|extension| extension["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("{name} is missing from {listed}"))
    };

    // The wire word, not the table's: `mix runtime ext list` prints "module" for a person.
    assert_eq!(of("xdebug")["linkage"], "shared");
    assert_eq!(of("xdebug")["enabled"], false, "{listed}");
    assert_eq!(of("xdebug")["source"], "build_default");

    // --- the set is on disk, generated by the install and by nobody else -------------------------
    let conf_d = home
        .path()
        .join("etc")
        .join("php")
        .join(VERSION)
        .join("conf.d");

    assert!(
        conf_d.join("00-mixengine.ini").is_file(),
        "installing a PHP did not give it an ini set: {}\n{}",
        conf_d.display(),
        home.daemon_log()
    );
    assert!(
        !conf_d.join("90-xdebug.ini").exists(),
        "an extension the build leaves off has a file switching it on"
    );

    // --- both consumers, and they agree -----------------------------------------------------------
    //
    // **The assertion this whole suite exists for.** `php -m` on a terminal and
    // `get_loaded_extensions()` in a pool are the two answers a person compares when a project works
    // in one and not the other.
    let started = home.mix(&["service", "start", &pool, "--json"]);
    assert!(
        started.status.success(),
        "the pool would not start
--- stderr ---
{}
--- daemon ---
{}",
        String::from_utf8_lossy(&started.stderr),
        home.daemon_log()
    );
    let started = json(&started);
    assert_eq!(
        started["complete"],
        true,
        "{started}\n{}",
        home.daemon_log()
    );

    let script = home.path().join("www").join("loaded.php");
    std::fs::create_dir_all(script.parent().expect("a parent")).expect("a document root");
    std::fs::write(
        &script,
        b"<?php echo implode(\",\", get_loaded_extensions()), \"\\n\";",
    )
    .expect("a script to serve");

    let terminal = through_the_terminal(&home);
    let served = through_the_pool(&listen, &script);
    agree(&terminal, &served, "before anything was turned round");

    assert!(
        !terminal.contains("xdebug"),
        "xdebug is loaded although nothing switched it on: {terminal:?}"
    );

    // The dev-tuned block reached PHP as well, which is the other half of `00-mixengine.ini`.
    let settings = std::process::Command::new(
        home.path()
            .join("bin")
            .join(format!("php{}", std::env::consts::EXE_SUFFIX)),
    )
    .args(["-r", "echo ini_get('memory_limit'), \"\\n\";"])
    .env("MIXENGINE_HOME", home.path())
    .output()
    .expect("php runs through the shim");
    assert!(
        String::from_utf8_lossy(&settings.stdout).contains("512M"),
        "the generated settings did not reach PHP: {}",
        String::from_utf8_lossy(&settings.stdout)
    );

    // --- turned on, and both sides move -----------------------------------------------------------
    let changed = json(&home.mix(&[
        "runtime", "ext", "enable", "xdebug", "--php", VERSION, "--json",
    ]));
    assert_eq!(changed["extension"]["enabled"], true, "{changed}");
    assert_eq!(changed["extension"]["source"], "user");
    assert!(
        conf_d.join("90-xdebug.ini").is_file(),
        "xdebug was enabled and nothing generated its file"
    );

    // A terminal is a new process every time, so it needs no reload at all — which is the difference
    // between the two consumers, and is why the pool is polled and this is not.
    let terminal = through_the_terminal(&home);
    assert!(
        terminal.contains("xdebug"),
        "the generated `zend_extension` line did not load xdebug — a `zend_extension` PHP cannot \
         load is a startup warning rather than a refusal to start, which is exactly the failure \
         this suite is here to catch: {terminal:?}"
    );

    // **What the pool did is the daemon's answer, and the suite obeys it rather than guessing.**
    match changed["pool"].as_str() {
        Some("reloaded") => {}

        // No signal to send: the running pool is still on the previous set, out loud, and what
        // clears it is a restart nobody but the user asks for.
        Some("restart_required") => {
            let restarted = json(&home.mix(&["service", "restart", &pool, "--json"]));
            assert_eq!(
                restarted["complete"],
                true,
                "{restarted}\n{}",
                home.daemon_log()
            );
        }

        other => panic!(
            "a pool that is running answered {other:?}\n{}",
            home.daemon_log()
        ),
    }

    let served = eventually(&listen, &script, |loaded| loaded.contains("xdebug"));
    agree(&terminal, &served, "after xdebug was turned on");

    // --- turned off again, and the file goes with it ----------------------------------------------
    let changed = json(&home.mix(&[
        "runtime", "ext", "disable", "xdebug", "--php", VERSION, "--json",
    ]));
    assert_eq!(changed["extension"]["enabled"], false, "{changed}");
    assert_eq!(
        changed["extension"]["source"], "build_default",
        "a choice that agrees with the build is forgotten rather than stored"
    );
    assert!(
        !conf_d.join("90-xdebug.ini").exists(),
        "xdebug was turned off and its file went on loading it — the sweep did not run"
    );

    if changed["pool"] == "restart_required" {
        home.mix(&["service", "restart", &pool, "--json"]);
    }

    let terminal = through_the_terminal(&home);
    assert!(!terminal.contains("xdebug"), "{terminal:?}");

    let served = eventually(&listen, &script, |loaded| !loaded.contains("xdebug"));
    agree(&terminal, &served, "after xdebug was turned off again");

    // --- the two refusals, against a real build ---------------------------------------------------
    let compiled_in = listed["extensions"]
        .as_array()
        .expect("a list")
        .iter()
        .find(|extension| extension["linkage"] == "static")
        .map(|extension| extension["name"].as_str().unwrap_or_default().to_owned())
        .expect("every PHP compiles something in");

    let refused = home.mix(&[
        "runtime",
        "ext",
        "disable",
        &compiled_in,
        "--php",
        VERSION,
        "--json",
    ]);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("compiled into"),
        "a refusal has to say that a different build is what it would take: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    let unknown = home.mix(&[
        "runtime", "ext", "enable", "swoole", "--php", VERSION, "--json",
    ]);
    assert!(
        !unknown.status.success(),
        "a name this build has never heard of was written down: {unknown:?}"
    );

    // --- and the set goes with the PHP it belongs to ----------------------------------------------
    let stopped = json(&home.mix(&["service", "stop", &pool, "--json"]));
    assert_eq!(
        stopped["complete"],
        true,
        "{stopped}\n{}",
        home.daemon_log()
    );

    let uninstalled = home.mix(&["runtime", "uninstall", "php", VERSION, "--json"]);
    assert!(
        uninstalled.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&uninstalled.stderr),
        home.daemon_log()
    );

    assert!(
        !home.path().join("etc").join("php").join(VERSION).exists(),
        "the generated ini set outlived the PHP it described"
    );
}
