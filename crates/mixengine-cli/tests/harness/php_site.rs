//! A home serving real PHP through a real front end — roadmap task **T72a**.
//!
//! **The arrangement nothing in this repository had until now.** `caddy.rs` proves a rendering the
//! server accepts, `php_fpm.rs` proves a pool that executes a script, and neither of them ever made
//! an HTTP request that reached PHP *through* Caddy. Two suites need exactly that and would
//! otherwise each build it: the one that proves a pool's status page cannot be asked for from
//! outside, and the cold-path budget.
//!
//! **One PHP or several, and the versions are read off the binaries rather than written down.**
//! `MIXENGINE_PHP_RUNTIMES` holds one or more unpacked PHP directories, separated the way this
//! system separates a `PATH`; `MIXENGINE_PHP_RUNTIME` is read as a list of one when it is not set,
//! which is what the `test` job already provides. Asking `php --version` for the number means an
//! index entry describes the artifact it is actually pointing at — a suite that hard-coded a version
//! beside a directory would publish 8.3.33 for whatever somebody unpacked there.

use std::path::{Path, PathBuf};

use mixengine_testkit::{FakePackage, MockRegistry, Packed, Packing};
use serde_json::Value;

use super::frontend::{CADDY, FrontEnd, free_port};
use super::{Home, json};

/// Where one or more unpacked PHPs are, as the CI step and a developer both set it.
const RUNTIMES: &str = "MIXENGINE_PHP_RUNTIMES";

/// The single-PHP variable every other suite already uses, read as a list of one.
const RUNTIME: &str = "MIXENGINE_PHP_RUNTIME";

/// A home with a front end and one pool per PHP, each with a site in front of it.
pub(crate) struct Served {
    /// The home, for `mix` and for `daemon_log`.
    pub home: Home,

    /// Held so the daemon outlives the fixture's caller rather than being reaped at the end of
    /// `served`.
    pub _daemon: super::Daemon,

    /// As `_daemon`: the registry serves the archives the installs read.
    pub _registry: MockRegistry,

    /// The port the front end answers on.
    pub port: u16,

    /// One per PHP, in the order the versions were given: the pool's service id and the domain of
    /// the site in front of it.
    pub sites: Vec<Site>,
}

/// One PHP, and what was built on top of it.
#[derive(Debug, Clone)]
pub(crate) struct Site {
    /// The version the index published, read off the binary itself.
    pub version: String,

    /// `php-fpm@<version>` — created by the install's own hook, not by this fixture.
    pub pool: String,

    /// The `Host` a request has to carry to reach it.
    pub domain: String,

    /// What this site's `index.php` prints, which is unique per site so a response cannot be
    /// mistaken for another site's.
    pub says: String,
}

/// Every PHP this machine was given, as directories.
///
/// # Panics
///
/// When neither variable is set, naming both and what fetches them — a suite that quietly returned
/// nothing would be a green run that proved nothing.
pub(crate) fn runtimes() -> Vec<PathBuf> {
    let listed = std::env::var_os(RUNTIMES)
        .or_else(|| std::env::var_os(RUNTIME))
        .unwrap_or_else(|| {
            panic!(
                "neither {RUNTIMES} nor {RUNTIME} is set, so there is no PHP to serve. The `php` \
                 steps in .github/workflows/ci.yml fetch them; by hand, unpack any PHP from \
                 mixengine-packages' releases and point {RUNTIME} at the directory."
            )
        });

    std::env::split_paths(&listed).collect()
}

/// What version the PHP in `root` actually is, asked of the binary.
///
/// **Asked rather than derived from the directory's name**, which is a developer's choice and can
/// say anything. The index entry has to describe the artifact it points at, and `runtime install`
/// creates a pool named after the version the index published.
fn version_of(root: &Path) -> String {
    let php = ["bin/php", "php"]
        .iter()
        .map(|relative| root.join(format!("{relative}{}", std::env::consts::EXE_SUFFIX)))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("{} holds no php binary", root.display()));

    let said = std::process::Command::new(&php)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("{} could not be run: {error}", php.display()));

    let first = String::from_utf8_lossy(&said.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();

    // `PHP 8.3.33 (cli) (built: …)` — the second word, and nothing about the rest of the line is
    // this fixture's business.
    first
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("`php --version` said something unreadable: {first}"))
        .to_owned()
}

/// What the artifact publishes, as an index entry says it.
///
/// **Probed rather than written down**, because the layout is the publisher's: `mixengine-packages`
/// puts the Unix binaries under `bin/` and `sbin/` and the Windows ones at the root. The same probe
/// `php_fpm.rs` makes, and for its reason.
fn provides(root: &Path) -> serde_json::Map<String, Value> {
    let mut found = serde_json::Map::new();

    for (name, candidates) in [
        ("php", ["bin/php", "php"].as_slice()),
        ("php-fpm", ["sbin/php-fpm", "bin/php-fpm"].as_slice()),
        ("php-cgi", ["php-cgi", "bin/php-cgi"].as_slice()),
    ] {
        for candidate in candidates {
            let relative = format!("{candidate}{}", std::env::consts::EXE_SUFFIX);

            if root.join(&relative).is_file() {
                found.insert(name.to_owned(), Value::String(relative));
                break;
            }
        }
    }

    let sapi = if cfg!(windows) { "php-cgi" } else { "php-fpm" };
    assert!(
        found.contains_key(sapi),
        "{} publishes no {sapi}, so there is nothing here for a pool to run — this needs the PHP \
         mixengine-packages builds, not a system one",
        root.display()
    );

    found
}

/// One index entry, for a package or a runtime.
fn entry(kind: &str, version: &str, packed: &Packed, url: &str, provides: Value) -> Value {
    serde_json::json!({
        "kind": kind,
        "version": version,
        "channel": "stable",
        "artifacts": [{
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "url": url,
            "sha256": packed.sha256,
            "size": packed.size(),
            "provides": provides,
        }],
    })
}

/// How this system packs an archive.
fn packing() -> Packing {
    if cfg!(windows) {
        Packing::Zip
    } else {
        Packing::TarZst
    }
}

/// A home with Caddy and every given PHP installed in it, a site per PHP, and the front end running.
///
/// **One registry publishing everything**, rather than a registry per artifact: a daemon reads one
/// index, and two would mean restarting it between installs.
///
/// The front end is left **running** and the pools are left **started**, which is the state both
/// callers need: one is about to ask a site for something, and the other is about to wait for the
/// sweeper to stop the pools.
pub(crate) async fn served(roots: &[PathBuf]) -> Served {
    assert!(!roots.is_empty(), "a home with no PHP serves nothing");

    let (port, control) = (free_port(), free_port());

    let caddy = CADDY.pack();
    let registry = MockRegistry::start(&serde_json::json!({
        "schema": 1, "generated_at": "2026-08-30T06:55:12Z", "packages": []
    }))
    .await;

    let mut packages = vec![{
        let url = registry.publish_asset(&caddy.path(), caddy.bytes.clone());
        let mut provides = serde_json::Map::new();
        provides.insert(
            CADDY.package.to_owned(),
            Value::String(format!("{}{}", CADDY.package, std::env::consts::EXE_SUFFIX)),
        );

        entry(
            CADDY.package,
            CADDY.version,
            &caddy,
            &url,
            Value::Object(provides),
        )
    }];

    let mut versions = Vec::with_capacity(roots.len());

    for root in roots {
        let version = version_of(root);
        let packed = FakePackage::new(packing())
            .directory(root)
            .build(&format!("php-{version}"));
        let url = registry.publish_asset(&packed.path(), packed.bytes.clone());

        packages.push(entry(
            "php",
            &version,
            &packed,
            &url,
            Value::Object(provides(root)),
        ));
        versions.push(version);
    }

    registry.publish(&serde_json::json!({
        "schema": 1,
        "generated_at": "2026-08-30T06:55:12Z",
        "packages": packages,
    }));

    let home = Home::new();
    let daemon = home.start_daemon_reading_index(&registry.url(), registry.public_key());

    let installed =
        json(&home.mix(&["package", "install", CADDY.package, CADDY.version, "--json"]));
    assert_eq!(
        installed["state"],
        "succeeded",
        "{installed}\n{}",
        home.daemon_log()
    );

    let created = json(&home.mix(&[
        "service",
        "create",
        CADDY.package,
        CADDY.version,
        "--port",
        &port.to_string(),
        "--json",
    ]));
    assert_eq!(
        created["service"]["id"],
        CADDY.package,
        "{created}\n{}",
        home.daemon_log()
    );

    // The admin endpoint off its default, for `declared`'s reason: a developer running this may well
    // have a Caddy of their own on 2019, and a suite that took it over is one that stops their work.
    mixengine_testkit::declare::reconfigure(
        &home.database_file(),
        CADDY.package,
        &(CADDY.alone)(control),
    )
    .await;

    let mut sites = Vec::with_capacity(versions.len());

    for version in versions {
        let installed = json(&home.mix(&["runtime", "install", "php", &version, "--json"]));
        assert_eq!(
            installed["state"],
            "succeeded",
            "{installed}\n{}",
            home.daemon_log()
        );

        sites.push(site(&home, &version));
    }

    let started = json(&home.mix(&["service", "start", CADDY.package, "--json"]));
    assert_eq!(
        started["complete"],
        true,
        "the front end has to be serving for any of this to mean anything: {started}\n{}",
        home.daemon_log()
    );

    for site in &sites {
        let started = json(&home.mix(&["service", "start", &site.pool, "--json"]));
        assert_eq!(
            started["complete"],
            true,
            "{started}\n{}",
            home.daemon_log()
        );
    }

    Served {
        home,
        _daemon: daemon,
        _registry: registry,
        port,
        sites,
    }
}

/// A project, a document root with an `index.php` in it, and a site pointed at this version's pool.
///
/// The directory is created under the home rather than in a `tempfile::TempDir`, so it lives exactly
/// as long as the home does: a doc root swept away while Caddy is still serving it is a 404 nobody
/// asked for.
fn site(home: &Home, version: &str) -> Site {
    let name = format!("php{}", version.replace('.', ""));
    let domain = format!("{name}.test");
    let says = format!("served by {version}");

    let root = home.path().join("projects").join(&name);
    std::fs::create_dir_all(&root).expect("a document root");
    std::fs::write(root.join("index.php"), format!("<?php echo \"{says}\";\n"))
        .expect("an index.php");

    let root_arg = root.display().to_string();
    home.mix(&["project", "create", &root_arg, "--name", &name]);

    let pool = format!("php-fpm@{version}");
    let created = home.mix_in(
        &root,
        &[],
        &[
            "site", "create", "--domain", &domain, "--kind", "php-fpm", "--pool", &pool, "--json",
        ],
    );
    assert!(
        created.status.success(),
        "a site for {version} was refused: {}\n{}",
        super::stderr(&created),
        home.daemon_log()
    );

    Site {
        version: version.to_owned(),
        pool,
        domain,
        says,
    }
}

/// The `FrontEnd` this fixture serves through, re-exported so a caller need not name two modules.
pub(crate) const FRONT: &FrontEnd = &CADDY;
