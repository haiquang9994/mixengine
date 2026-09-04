//! `mix self-update` end to end — roadmap task **T88**.
//!
//! **The one test that can exercise the whole sequence**, and it has to be an end-to-end one for a
//! reason no unit test can get around: what an update is *about* is a running daemon replacing the
//! binaries it and its client are executing, and then a different process coming up out of them.
//! Every part of that is proved where it lives — the feed's verification in `core::index`, the swap
//! and its rollback in `core::updates::apply`, the refusals in `api/rpc.rs` — and none of it proves
//! that a person typing two words ends up running the new build with the same services up.
//!
//! **So this copies `mix` and `mixengined` into a directory of its own** and drives them from there.
//! A test that swapped the binaries in `target/debug` would replace the ones `cargo test` is running
//! the rest of the suite out of.
//!
//! **The payload is this build's own binaries under a version the feed calls newer**, which makes
//! the last assertion possible rather than being a corner cut: a test cannot compile a binary with a
//! different `CARGO_PKG_VERSION`, so the release that comes up is the one that was already running —
//! and the post-restart check is written to notice exactly that and stop offering the release. The
//! payload being the same version is therefore a **pass** of this test rather than a hole in it.
//!
//! Nothing here is `#[ignore]`d for the network: the feed is served in-process and the payload is a
//! served asset. What *is* ignored in a release build is the half that needs a service, for
//! `daemon.rs`' reason — the `fakeservice` recipe is compiled into debug builds alone.

mod harness;

use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use harness::{Home, stdout};
use mixengine_testkit::{FakePackage, MockRegistry, Packing, Service};

/// The version the feed offers. Higher than anything this product has published, so the offer
/// decision's "is it newer" is satisfied by a payload that is really this build.
const OFFERED: &str = "99.0.0";

/// The three names a release is made of.
const BINARIES: [&str; 3] = ["mix", "mixengined", "mixengine-elevate"];

/// What the helper's stand-in holds, so a test can prove it was not replaced.
const HELPER_STUB: &[u8] = b"not the elevated helper, and not to be replaced";

/// A copy of this build, installed where a test may replace it.
struct Installed {
    /// The directory holding the three binaries.
    directory: PathBuf,

    /// Where the feed is served, and what verifies it.
    feed: Option<(String, String)>,

    /// Kept so the directory outlives the test.
    _root: tempfile::TempDir,
}

impl Installed {
    /// Copy `mix` and `mixengined` out of `target/`, and put a stand-in beside them for the helper.
    ///
    /// **A stand-in and not the real helper**, because what is being asserted about it is that
    /// nothing touched it: a byte-for-byte comparison against a file whose contents are known is the
    /// cheapest form of that, and this test has no business installing a privileged binary anywhere.
    fn here() -> Self {
        let root = tempfile::tempdir().expect("a temporary directory");
        let directory = root.path().join("installed");
        std::fs::create_dir_all(&directory).expect("an install directory");

        std::fs::copy(mix_binary(), directory.join(named("mix"))).expect("a copy of mix");
        std::fs::copy(daemon_binary(), directory.join(named("mixengined")))
            .expect("a copy of mixengined");
        std::fs::write(directory.join(named("mixengine-elevate")), HELPER_STUB)
            .expect("a stand-in for the helper");

        Self {
            directory,
            feed: None,
            _root: root,
        }
    }

    /// The path of one of the three, as it is installed.
    fn binary(&self, name: &str) -> PathBuf {
        self.directory.join(named(name))
    }

    /// What is on disk for one of them now.
    fn contents(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.binary(name))
            .unwrap_or_else(|error| panic!("read the installed {name}: {error}"))
    }

    /// Point every process this fixture starts at a feed this test is serving.
    ///
    /// **Through the environment and not through `--update-url`**, and the test found the reason:
    /// the daemon `mix` relaunches at the end of an update is spawned by `Autostart::run`, which
    /// passes `--detach` and `--home` and nothing else. Flags reach that child only by being in the
    /// environment it inherits — which is also how somebody running against a staging feed would do
    /// it, since the daemon they are updating is not the daemon they started.
    fn reading(mut self, url: &str, key: &str) -> Self {
        self.feed = Some((url.to_owned(), key.to_owned()));
        self
    }

    /// Run the installed `mix`, from the installed directory.
    ///
    /// **From that directory**, which is the whole point: `Autostart` looks for `mixengined` beside
    /// `current_exe()`, so the daemon this command relaunches is the one it just replaced rather
    /// than whatever `cargo` built.
    fn mix(&self, home: &Home, args: &[&str]) -> Output {
        let mut command = Command::new(self.binary("mix"));
        command
            .args(args)
            .arg("--home")
            .arg(home.path())
            .current_dir(&self.directory);

        self.point_at_the_feed(&mut command);

        command.output().expect("the installed mix runs")
    }

    /// Start the installed daemon against the feed this test is serving.
    fn start_daemon(&self, home: &Home) -> Running {
        let mut command = Command::new(self.binary("mixengined"));
        command
            .arg("--home")
            .arg(home.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        self.point_at_the_feed(&mut command);

        let child = command.spawn().expect("the installed daemon runs");

        home.wait_until_listening();

        Running(child)
    }

    fn point_at_the_feed(&self, command: &mut Command) {
        if let Some((url, key)) = &self.feed {
            command.env("MIXENGINE_UPDATE_URL", url);
            command.env("MIXENGINE_UPDATE_KEY", key);
        }
    }
}

/// A daemon this test started, killed when the test ends however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What a binary is called on this system.
fn named(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

fn mix_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mix"))
}

fn daemon_binary() -> PathBuf {
    let daemon = mix_binary()
        .parent()
        .expect("the test binary has a directory")
        .join(named("mixengined"));

    assert!(
        daemon.is_file(),
        "{} is not there — this test drives a real daemon, so run `cargo test --workspace` rather \
         than `cargo test -p mixengine-cli`",
        daemon.display()
    );

    daemon
}

/// A payload archive holding the installed binaries, laid out the way `packaging/` lays one out.
///
/// **One top-level `mixengine/` directory**, which is what every packaging script writes and what
/// the `provides` map below describes — `packaging/feed.sh` computes exactly this by opening the
/// archive it is describing.
fn payload(installed: &Installed) -> mixengine_testkit::Packed {
    let root = tempfile::tempdir().expect("a temporary directory");
    let inside = root.path().join("mixengine");
    std::fs::create_dir_all(&inside).expect("the payload's one directory");

    for name in BINARIES {
        std::fs::copy(installed.binary(name), inside.join(named(name)))
            .unwrap_or_else(|error| panic!("copy {name} into the payload: {error}"));
    }

    // `.tar.gz` on every system, which the client unpacks on every system. The real Windows payload
    // is a `.zip`; what differs between the two is the executable bit, which is a property of the
    // machine unpacking rather than of this sequence.
    FakePackage::new(Packing::TarGz)
        .directory(root.path())
        .build(&format!("mixengine-{OFFERED}-test"))
}

/// The `latest.json` a release publishes, for this machine.
fn feed(url: &str, packed: &mixengine_testkit::Packed) -> serde_json::Value {
    let provides: serde_json::Map<String, serde_json::Value> = BINARIES
        .iter()
        .map(|name| {
            (
                (*name).to_owned(),
                serde_json::Value::String(format!("mixengine/{}", named(name))),
            )
        })
        .collect();

    serde_json::json!({
        "schema": 1,
        "generated_at": "2026-09-05T09:12:00Z",
        "version": OFFERED,
        "published_at": "2026-09-05T09:12:00Z",
        "notes": "feat(cli): mix self-update",
        "notes_url": "https://example.invalid/releases/v99.0.0",
        "artifacts": [{
            "os": os_name(),
            "arch": arch_name(),
            "url": url,
            "sha256": packed.sha256,
            "size": packed.bytes.len(),
            "provides": provides,
        }]
    })
}

/// This machine's operating system, as the feed spells it.
fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        other => other,
    }
}

/// This machine's architecture, as the feed spells it.
fn arch_name() -> &'static str {
    std::env::consts::ARCH
}

/// The whole sequence: offer, consent, download, verify, smoke, stop, swap, relaunch, restore.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    not(debug_assertions),
    ignore = "the fakeservice recipe is compiled into debug builds only"
)]
async fn an_update_replaces_the_binaries_relaunches_and_starts_what_was_running() {
    let installed = Installed::here();
    let packed = payload(&installed);

    let registry = MockRegistry::start(&serde_json::json!({"schema": 1})).await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());
    registry.publish(&feed(&url, &packed));

    let installed = installed.reading(&registry.url(), registry.public_key());
    let home = Home::new();
    let _daemon = installed.start_daemon(&home);

    // `block_in_place`, because `Home::declare` builds a current-thread runtime of its own and this
    // test is already inside one — `MockRegistry` needs a runtime, and a runtime cannot be started
    // from a thread that is driving one.
    tokio::task::block_in_place(|| home.declare(&[Service::new("fakeservice@main")]));

    let started = installed.mix(&home, &["service", "start", "fakeservice@main", "--json"]);
    assert!(started.status.success(), "{}", stdout(&started));

    let helper_before = installed.contents("mixengine-elevate");

    let updated = installed.mix(&home, &["self-update", "--yes", "--json"]);
    assert!(
        updated.status.success(),
        "self-update failed\n--- stdout ---\n{}\n--- daemon.log ---\n{}",
        stdout(&updated),
        home.daemon_log()
    );

    // **What moved is read out of the daemon's own report and not off the disk**, and the first
    // version of this test got that wrong — it looked for the `.old` files, which is a race it
    // cannot win: the daemon that comes up next removes the ones it can, and on Windows that is
    // `mixengined.exe.old` but not `mix.exe.old`, because the `mix` running the update still holds
    // its own image open. The bytes cannot answer either, since the payload *is* these binaries.
    //
    // `UpdateApplied` is what can answer, and it is the honest place to ask: it is the daemon saying
    // which names it replaced and which it kept, written before it exited.
    let applied: serde_json::Value =
        serde_json::from_slice(&updated.stdout).unwrap_or_else(|error| {
            panic!(
                "mix --json prints one JSON document: {error}\n{}",
                stdout(&updated)
            )
        });

    assert_eq!(applied["from"], env!("CARGO_PKG_VERSION"), "{applied}");
    assert_eq!(applied["to"], OFFERED, "{applied}");
    assert_eq!(
        applied["replaced"],
        serde_json::json!(["mix", "mixengined"]),
        "the swap replaced something other than the two binaries an update replaces: {applied}"
    );
    assert_eq!(
        applied["kept"],
        serde_json::json!(["mixengine-elevate"]),
        "{applied}"
    );
    assert_eq!(
        applied["restarting"],
        serde_json::json!(["fakeservice@main"]),
        "the update did not record the service it stopped: {applied}"
    );

    // **And the helper did not.** `.claude/features/updates.md`'s single most important rule, as the
    // one assertion that can be made about it from outside: an auto-updated binary that runs as
    // root, with no OS signature, is a local privilege-escalation vector — so T88a replaces it,
    // inside an elevation prompt, and this task does not.
    assert_eq!(
        installed.contents("mixengine-elevate"),
        helper_before,
        "the elevated helper was replaced by an ordinary update"
    );
    assert!(
        !installed
            .directory
            .join(format!("{}{}", named("mixengine-elevate"), ".old"))
            .exists(),
        "the helper was renamed out of the way, which is a swap that should never have started"
    );

    // **A daemon is answering again**, and it is the one `mix` started rather than the one that was
    // replaced. `mix status` succeeding is the whole of that claim: the endpoint is bound, the
    // handshake passed, and the home is this one.
    let after = installed.mix(&home, &["status", "--no-autostart", "--json"]);
    assert!(
        after.status.success(),
        "no daemon is answering after the update\n--- stdout ---\n{}\n--- daemon.log ---\n{}",
        stdout(&after),
        home.daemon_log()
    );

    // **What was running is running.** The restore is the daemon's own pass over the list the stop
    // produced, so what this asserts is the property `.claude/features/updates.md` states:
    // *accepting an update restarts exactly the services that were running before it*.
    let restarted = wait_until_running(&installed, &home);
    assert!(
        restarted,
        "fakeservice@main was not started again after the update\n--- daemon.log ---\n{}",
        home.daemon_log()
    );

    // **And the release is not offered again.** The payload's real version is this build's, not the
    // `99.0.0` the feed declared, so the post-restart check found a mismatch and wrote the release
    // off — which is what turns a mislabelled release into one pointless update instead of one every
    // 24 h for ever.
    let status = installed.mix(&home, &["self-update", "--check", "--json"]);
    assert!(status.status.success(), "{}", stdout(&status));

    let answer: serde_json::Value =
        serde_json::from_slice(&status.stdout).expect("mix --json prints JSON");
    assert_eq!(answer["offered"], false, "{answer}");
    assert!(
        answer["because"]
            .as_str()
            .is_some_and(|because| because.contains("skipped")),
        "the mislabelled release should have been written off, and was not: {answer}"
    );

    // Stopped here rather than left to `Drop`: the daemon `mix` relaunched is nobody's child, and a
    // temporary home cannot be removed on Windows while a process holds its log file open.
    let _ = installed.mix(&home, &["daemon", "stop"]);
}

/// Poll until the service is running again, or give up.
///
/// Polled rather than read once: the restore is spawned by the new daemon's start rather than
/// awaited by it — the endpoint is bound first, on the reasoning every other start-time pass in
/// `main.rs` gives — so a reading taken the instant `mix status` answers is a reading taken too
/// early.
fn wait_until_running(installed: &Installed, home: &Home) -> bool {
    let deadline = std::time::Instant::now() + mixengine_testkit::home::STARTUP;

    while std::time::Instant::now() < deadline {
        let listed = installed.mix(home, &["service", "status", "fakeservice@main", "--json"]);

        if listed.status.success()
            && serde_json::from_slice::<serde_json::Value>(&listed.stdout)
                .ok()
                .and_then(|summary| summary["state"].as_str().map(str::to_owned))
                .is_some_and(|state| state == "running" || state == "ready")
        {
            return true;
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    false
}

/// A machine this release has no build for is told which pair, and nothing is downloaded.
///
/// **Its own test rather than a branch of the one above**, because it is the case a release that
/// skipped an architecture produces — five build legs and six possible machines — and because it is
/// the one refusal that has to happen without a daemon ever reading a payload.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_with_no_build_for_this_machine_is_not_offered() {
    let installed = Installed::here();
    let packed = payload(&installed);

    let registry = MockRegistry::start(&serde_json::json!({"schema": 1})).await;
    let url = registry.publish_asset(&packed.path(), packed.bytes.clone());

    let mut document = feed(&url, &packed);
    // The one machine nobody is running this suite on.
    document["artifacts"][0]["os"] = serde_json::json!("linux");
    document["artifacts"][0]["arch"] = serde_json::json!(match arch_name() {
        "x86_64" => "aarch64",
        _ => "x86_64",
    });
    registry.publish(&document);

    let installed = installed.reading(&registry.url(), registry.public_key());
    let home = Home::new();
    let _daemon = installed.start_daemon(&home);

    let checked = installed.mix(&home, &["self-update", "--check", "--json"]);
    assert!(checked.status.success(), "{}", stdout(&checked));

    let answer: serde_json::Value =
        serde_json::from_slice(&checked.stdout).expect("mix --json prints JSON");

    // The release is *reported* and not offered, which is the distinction that lets a client say
    // "99.0.0 exists, and there is no build for this machine" rather than "no update".
    assert_eq!(answer["available"]["version"], OFFERED, "{answer}");
    assert_eq!(answer["offered"], false, "{answer}");
    assert!(
        answer["because"]
            .as_str()
            .is_some_and(|because| because.contains("no build for this machine")),
        "{answer}"
    );

    let _ = installed.mix(&home, &["daemon", "stop"]);
}

/// A payload whose bytes are not the ones the signed feed named is refused, and nothing is swapped.
///
/// **The acceptance criterion `.claude/features/updates.md` states first**: *a tampered artifact
/// fails the minisign check and is refused, with the reason shown*. The check is a SHA-256 inside a
/// minisign-signed document, so tampering with the payload is what an attacker who can answer the
/// URL actually gets to do — and this is that attempt.
#[tokio::test(flavor = "multi_thread")]
async fn a_payload_that_is_not_what_the_feed_named_is_refused_and_nothing_is_replaced() {
    let installed = Installed::here();
    let packed = payload(&installed);

    let registry = MockRegistry::start(&serde_json::json!({"schema": 1})).await;
    // Served under the name the feed will carry, with contents that are not what it hashes to.
    let url = registry.publish_asset(&packed.path(), b"not the payload at all".to_vec());
    registry.publish(&feed(&url, &packed));

    let installed = installed.reading(&registry.url(), registry.public_key());
    let home = Home::new();
    let _daemon = installed.start_daemon(&home);

    let before: Vec<Vec<u8>> = BINARIES
        .iter()
        .map(|name| installed.contents(name))
        .collect();

    let updated = installed.mix(&home, &["self-update", "--yes"]);
    assert!(
        !updated.status.success(),
        "a payload that is not the one the feed named was accepted\n{}",
        stdout(&updated)
    );

    for (name, contents) in BINARIES.iter().zip(before) {
        assert_eq!(
            installed.contents(name),
            contents,
            "{name} was replaced by an update that should have been refused"
        );
        assert!(
            !installed
                .directory
                .join(format!("{}{}", named(name), ".old"))
                .exists(),
            "{name} was renamed out of the way by an update that never got as far as a swap"
        );
    }

    // And the daemon that refused it is still the daemon: a refusal before the stop leaves a home
    // exactly as it was.
    let after = installed.mix(&home, &["status", "--no-autostart"]);
    assert!(after.status.success(), "{}", stdout(&after));

    let _ = installed.mix(&home, &["daemon", "stop"]);
}
