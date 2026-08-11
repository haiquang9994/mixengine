//! `mix` against a real `mixengined`, over a real endpoint.
//!
//! This is the test the whole of task T10 exists to pass, and none of it can be written any other
//! way: that the client and the daemon agree on which home they are talking about, that they agree
//! on the endpoint that home implies, and that a client which finds nothing there produces a daemon
//! rather than an error message. The pieces are mocked nowhere, because every one of those claims is
//! about two operating-system processes.
//!
//! It is also the guard on the one thing `mix` duplicates rather than shares. The daemon computes
//! `run/` through `mixengine_core::Paths` and the client computes it in `home.rs`, because `core`
//! carries `sqlx` — see the note there. Nothing but a run like this one would notice the two drifting
//! apart: a `mix` that looked in the wrong place would simply autostart a second daemon, forever.
//!
//! Every test gets its own `MIXENGINE_HOME` in a `TempDir` **passed as `--home`** — rule 2 in
//! `.claude/standards/testing.md`. Nothing here touches the network; a Unix socket and a named pipe
//! are neither.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use mixengine_platform::ipc::Endpoint;
use serde_json::Value;
use tempfile::TempDir;

/// How long a daemon started in the foreground is given to bind its endpoint.
///
/// Generous, because the first start of a home creates its directory tree, runs the migrations and
/// opens SQLite — and because a loaded CI runner is the machine this has to be reliable on. It is a
/// ceiling and not a wait.
const STARTUP: Duration = Duration::from_secs(30);

/// How long a killed daemon is given to let go of its home before the directory is removed.
///
/// Not a correctness deadline — `TempDir` ignores a removal that fails, so the worst case is a
/// directory left in the system's temporary folder. It exists because a Windows daemon holds both
/// its lock file and its working directory open, so removing the home a moment after `taskkill`
/// would fail more often than not.
const SETTLE: Duration = Duration::from_millis(250);

/// A home directory that exists only for this test.
struct Home {
    dir: TempDir,
    endpoint: Endpoint,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temporary home");

        // Computed the way the *client* computes it. That is deliberate: the assertions below check
        // this against what the daemon reports, which is the way `mixengine_core::Paths` computes
        // it, and the two being the same string is the property this file exists to hold.
        let endpoint =
            Endpoint::in_run_dir(&dir.path().join("run")).expect("an endpoint for this home");

        Self { dir, endpoint }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Run `mix` against this home, to completion.
    fn mix(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mix"))
            .args(args)
            .arg("--home")
            .arg(self.path())
            .output()
            .expect("the mix binary runs")
    }

    /// Start a daemon in the foreground, as a service manager would, and wait until it answers.
    ///
    /// Killed when the returned handle drops. Nothing here uses `--detach`: a foreground daemon is
    /// this process's child, which is what makes it stoppable at the end of a test.
    fn start_daemon(&self) -> Daemon {
        let child = Command::new(daemon_binary())
            .arg("--home")
            .arg(self.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the daemon binary runs");

        let daemon = Daemon(child);
        self.wait_until_listening();
        daemon
    }

    /// The process answering for this home, if anything is.
    ///
    /// Asked of the endpoint rather than remembered from a spawn, because the daemon that has to be
    /// found is precisely the one nobody is holding: `mix` autostarts it, it is not this process's
    /// child, and the test that made it may have failed before it could say so.
    fn listening_pid(&self) -> Option<u32> {
        let output = self.mix(&["status", "--no-autostart", "--json"]);

        if !output.status.success() {
            return None;
        }

        serde_json::from_slice::<Value>(&output.stdout)
            .ok()?
            .pointer("/daemon/pid")?
            .as_u64()
            .map(|pid| pid as u32)
    }

    /// Poll the endpoint until something is behind it.
    ///
    /// A blocking dial rather than `mixengine_platform::ipc::Connection`, which needs a runtime:
    /// what is being waited for is that the endpoint exists at all, and `mix` is what proves it can
    /// be spoken to.
    fn wait_until_listening(&self) {
        let deadline = std::time::Instant::now() + STARTUP;

        while std::time::Instant::now() < deadline {
            if self.mix(&["status", "--no-autostart"]).status.success() {
                return;
            }

            std::thread::sleep(Duration::from_millis(25));
        }

        panic!(
            "no daemon answered on {} within {STARTUP:?}\n--- daemon.log ---\n{}",
            self.endpoint,
            std::fs::read_to_string(self.path().join("logs").join("daemon.log"))
                .unwrap_or_else(|error| format!("(unreadable: {error})"))
        );
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        // A daemon `mix` autostarted is not this process's child and outlives the test that made
        // it. Anything still holding this home has to go before the directory does, or the next run
        // of the suite finds a daemon serving a home that no longer exists — and here rather than
        // at the end of a test body because a failed assertion is exactly when one gets left
        // behind. `try_stop`, because a `Daemon` dropped a moment ago may already be gone and a
        // panic while unwinding aborts the whole run.
        if let Some(pid) = self.listening_pid() {
            try_stop(pid);
        }

        std::thread::sleep(SETTLE);
    }
}

/// A daemon this test started and is responsible for.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        // Killed rather than asked to stop: `daemon.shutdown` is task T9a's, and an interrupt cannot
        // be delivered to a child portably.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The `mixengined` built alongside this `mix`.
///
/// `CARGO_BIN_EXE_…` only names binaries of the *same* package, and the daemon is another one — so
/// it is found the way `mix` itself finds it at runtime, next to the client. That is not a
/// workaround so much as the same claim under test: `cargo test --workspace`, which is what CI runs
/// and what [CLAUDE.md](../../../CLAUDE.md) lists, builds both into one directory.
fn daemon_binary() -> PathBuf {
    let mix = PathBuf::from(env!("CARGO_BIN_EXE_mix"));
    let daemon = mix
        .parent()
        .expect("the test binary has a directory")
        .join(format!("mixengined{}", std::env::consts::EXE_SUFFIX));

    assert!(
        daemon.is_file(),
        "{} is not there — these tests drive a real daemon, so run `cargo test --workspace` \
         rather than `cargo test -p mixengine-cli`",
        daemon.display()
    );

    daemon
}

/// The JSON a successful `mix --json` printed.
fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "mix exited {}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "mix --json prints JSON on stdout: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// Stop a process this test is not the parent of. `false` if it was not there to stop.
fn try_stop(pid: u32) -> bool {
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("kill");
        command.arg(pid.to_string());
        command
    };

    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &pid.to_string(), "/F"]);
        command
    };

    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("this system can stop a process")
        .success()
}

#[test]
fn status_starts_a_daemon_for_a_home_that_has_none_and_then_describes_it() {
    let home = Home::new();

    // Milestone M0, in one line: nothing is running, nothing has been created, and a person types
    // `mix status`.
    let status = json(&home.mix(&["status", "--json"]));
    let daemon = &status["daemon"];

    // Stopped by `Home::drop`, which asks the endpoint who is answering rather than being told
    // here: an assertion below that fails would otherwise leave a daemon serving a home that is
    // about to be deleted.
    assert!(daemon["pid"].as_u64().is_some(), "{status}");

    assert_eq!(daemon["protocol"], 1);
    assert_eq!(status["client"]["protocol"], 1);
    assert_eq!(daemon["version"], status["client"]["version"]);

    // The two halves of the claim this file exists for. The daemon resolved its home and its
    // endpoint through `mixengine_core::Paths`; the client resolved both on its own and reached it.
    assert!(
        Path::new(daemon["home"].as_str().expect("a home")).ends_with(
            home.path()
                .file_name()
                .expect("the temporary home has a name")
        ),
        "the daemon is serving {} and the client asked about {}",
        daemon["home"],
        home.path().display()
    );
    assert_eq!(daemon["endpoint"], home.endpoint.to_string());
}

#[test]
fn status_talks_to_the_daemon_that_is_already_there_instead_of_starting_another() {
    let home = Home::new();
    let daemon = home.start_daemon();

    let status = json(&home.mix(&["status", "--json"]));

    // The single-instance lock would have caught a second daemon, but only after the fact and only
    // in a log nobody reads. What is asserted is the thing a user would notice: the answer came from
    // the process that was already running.
    assert_eq!(status["daemon"]["pid"], daemon.0.id());
}

#[test]
fn the_human_rendering_leads_with_the_state_and_the_home() {
    let home = Home::new();
    let _daemon = home.start_daemon();

    let output = home.mix(&["status"]);
    let rendered = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "{output:?}");
    assert!(rendered.starts_with("mixengined "), "{rendered}");
    assert!(rendered.contains("running (pid "), "{rendered}");
    assert!(
        rendered.contains(&home.endpoint.to_string()),
        "the endpoint is what tells one daemon from another: {rendered}"
    );
}

#[test]
fn no_autostart_answers_the_question_without_creating_anything() {
    let home = Home::new();
    let output = home.mix(&["status", "--no-autostart"]);

    // A non-zero exit, because a caller that asked about a daemon and found none needs to be able
    // to tell without parsing anything. The machine-readable half of the same answer is the code
    // below.
    assert!(!output.status.success());
    assert!(output.stdout.is_empty(), "{output:?}");

    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(
        complaint.contains("no MixEngine daemon is listening"),
        "{complaint}"
    );
    assert!(complaint.contains("--no-autostart"), "{complaint}");

    // The property the flag exists for: a monitoring check that asks whether MixEngine is running
    // must not be the thing that installs it.
    assert_eq!(
        std::fs::read_dir(home.path())
            .expect("the temporary home is readable")
            .count(),
        0,
        "asking about a daemon created something in {}",
        home.path().display()
    );
}

#[test]
fn a_failure_is_the_same_wire_error_a_daemon_would_have_sent() {
    let home = Home::new();
    let output = home.mix(&["status", "--json", "--no-autostart"]);

    assert!(!output.status.success());

    // On stderr, and not on stdout: a script redirecting stdout into a file gets a status object or
    // an empty file, never an error object where a status was meant to be.
    assert!(output.stdout.is_empty(), "{output:?}");

    let error: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|failure| {
        panic!(
            "mix --json fails in JSON too: {failure}\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });

    // The whole point of carrying the wire error through: this code is the same one a daemon would
    // have sent for the same situation, so nothing branches on which side produced it.
    assert_eq!(error["code"], "precondition_failed");
    assert!(error["hint"].is_string(), "{error}");
}

#[test]
fn an_empty_home_never_gets_as_far_as_meaning_the_real_one() {
    let output = Command::new(env!("CARGO_BIN_EXE_mix"))
        .args(["status", "--home", ""])
        .output()
        .expect("the mix binary runs");

    // `clap` gets there first and refuses the value outright, with its own usage exit code rather
    // than ours — checked here because that is the claim `core` and `home.rs` both rest on, and
    // neither of them can see it. The guard behind it stays: `home.rs` is reachable from a test and
    // from whatever a later command does with a home, and treating an empty override as "not given"
    // would point a sandbox run at the real install.
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");

    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("--home"), "{complaint}");
}
