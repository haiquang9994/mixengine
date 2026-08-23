//! What every end-to-end test of `mix` needs: a home, a daemon in it, and a way to run the client.
//!
//! It lives here rather than in `mixengine-testkit` because none of it is about MixEngine — it is
//! about *this* suite: finding the `mixengined` built beside this `mix`, and leaving nothing running
//! in a temporary directory that is about to be deleted. What is shared with the rest of the
//! workspace is underneath it, in `mixengine_testkit::Home`.

// Each integration test binary compiles this module separately, so anything `status.rs` uses and
// `service.rs` does not — or the other way round — is dead code in one of them. The alternative is
// two copies of the [`Drop`] below, which is the one piece here that must not be got wrong twice.
#![allow(dead_code)]

pub(crate) mod frontend;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::Duration;

use mixengine_testkit::home::STARTUP;
use mixengine_testkit::try_stop;
use serde_json::Value;

/// How long a killed daemon is given to let go of its home before the directory is removed.
///
/// Not a correctness deadline — `TempDir` ignores a removal that fails, so the worst case is a
/// directory left in the system's temporary folder. It exists because a Windows daemon holds both
/// its lock file and its working directory open, so removing the home a moment after `taskkill`
/// would fail more often than not.
const SETTLE: Duration = Duration::from_millis(250);

/// A home directory that exists only for this test, and a promise to leave nothing running in it.
///
/// A thin thing around `mixengine_testkit::Home`, which is where the directory and the endpoint come
/// from. What is added here is the [`Drop`] below, and it belongs to this suite rather than to the
/// fixture: a daemon `mix` autostarted is nobody's child, and only a test that autostarts one has to
/// go looking for it afterwards.
///
/// The endpoint the fixture computes is the *client's* answer — `run/` directly under the root —
/// which is exactly what makes it usable here: the assertions in `status.rs` check it against what
/// the daemon reports, and the two being the same string is the property that file exists to hold.
pub(crate) struct Home(mixengine_testkit::Home);

impl Home {
    pub(crate) fn new() -> Self {
        Self(mixengine_testkit::Home::new())
    }

    /// What is in the home, by file name, sorted — compare against
    /// [`mixengine_testkit::Home::SEEDED`] when the claim is "that command created nothing".
    pub(crate) fn contents(&self) -> Vec<String> {
        self.0.contents()
    }

    pub(crate) fn path(&self) -> &Path {
        self.0.path()
    }

    pub(crate) fn endpoint(&self) -> String {
        self.0.endpoint().to_string()
    }

    /// Whatever the daemon for this home has written to its own log.
    pub(crate) fn daemon_log(&self) -> String {
        self.0.daemon_log()
    }

    /// This home's database, for a test that writes a row itself.
    pub(crate) fn database_file(&self) -> PathBuf {
        self.0.database_file()
    }

    /// Give these services a `services` row, which is what makes them startable.
    ///
    /// The daemon renders each one into a configuration and a spec through its `fakeservice` recipe
    /// — see `mixengine_testkit::declare` — so what a test writes here is what the service will do.
    pub(crate) fn declare(&self, services: &[mixengine_testkit::Service]) {
        self.0.declare(services);
    }

    /// Run `mix` against this home, to completion.
    pub(crate) fn mix(&self, args: &[&str]) -> Output {
        self.try_mix(args).expect("the mix binary runs")
    }

    /// Run `mix` from a directory of its own, with these variables in its environment.
    ///
    /// The two things `mix runtime resolve` is only able to get wrong as a *client*: which directory
    /// it tells the daemon it is in, and whether it reads `MIXENGINE_PHP` at all. Neither is
    /// observable from a `mix` that inherits this test process's own directory and environment — and
    /// the variables go on the child rather than through `std::env::set_var`, which
    /// `.claude/standards/testing.md` forbids for the reason two tests in one binary would find out
    /// the hard way.
    pub(crate) fn mix_in(&self, cwd: &Path, environment: &[(&str, &str)], args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mix"));
        command
            .args(args)
            .arg("--home")
            .arg(self.path())
            .current_dir(cwd);

        for (name, value) in environment {
            command.env(name, value);
        }

        command.output().expect("the mix binary runs")
    }

    /// Run `mix` against this home with these bytes on its standard input.
    ///
    /// The one thing [`Home::mix`] cannot do: `Command::output` gives the child a closed stdin, so a
    /// command that asks a question reads end-of-file and never sees an answer. That case is worth
    /// testing on its own — it is what a cron job looks like — but so is the answer, and this is how
    /// a test types one.
    ///
    /// A pipe and not a terminal, which is the whole reason `mix elevation grant` decides on what it
    /// can *read* rather than on `IsTerminal`: a rule that only a console could satisfy would be a
    /// rule no test could reach.
    pub(crate) fn mix_answering(&self, answer: &str, args: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_mix"))
            .args(args)
            .arg("--home")
            .arg(self.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the mix binary runs");

        child
            .stdin
            .take()
            .expect("a piped stdin")
            .write_all(answer.as_bytes())
            .expect("mix reads what it was given");

        child.wait_with_output().expect("mix finishes")
    }

    /// The same, for the caller that is not allowed to panic. See [`Home::listening_pid`].
    fn try_mix(&self, args: &[&str]) -> Option<Output> {
        Command::new(env!("CARGO_BIN_EXE_mix"))
            .args(args)
            .arg("--home")
            .arg(self.path())
            .output()
            .ok()
    }

    /// Start a daemon in the foreground, as a service manager would, and wait until it answers.
    ///
    /// Killed when the returned handle drops. Nothing here uses `--detach`: a foreground daemon is
    /// this process's child, which is what makes it stoppable at the end of a test.
    pub(crate) fn start_daemon(&self) -> Daemon {
        self.spawn_daemon(&[])
    }

    /// The same, for a daemon that reads its package index from a registry this test is serving.
    pub(crate) fn start_daemon_reading_index(&self, url: &str, key: &str) -> Daemon {
        self.spawn_daemon(&["--index-url", url, "--index-key", key])
    }

    fn spawn_daemon(&self, arguments: &[&str]) -> Daemon {
        let mut command = Command::new(daemon_binary());
        command
            .arg("--home")
            .arg(self.path())
            .args(arguments)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let daemon = Daemon(command.spawn().expect("the daemon binary runs"));
        self.wait_until_listening();
        daemon
    }

    /// The process answering for this home, if anything is.
    ///
    /// Asked of the endpoint rather than remembered from a spawn, because the daemon that has to be
    /// found is precisely the one nobody is holding: `mix` autostarts it, it is not this process's
    /// child, and the test that made it may have failed before it could say so.
    ///
    /// **Every way of failing is `None`, including the client not running at all.** This is called
    /// from [`Drop`], which runs while a failed assertion is unwinding, and a panic there aborts the
    /// process — no per-test failure, and the tests after it never run. `try_mix` rather than `mix`
    /// for exactly that: "the daemon cannot be asked" and "nothing is listening" lead to the same
    /// place here, and only one of them is allowed to be loud.
    fn listening_pid(&self) -> Option<u32> {
        let output = self.try_mix(&["status", "--no-autostart", "--json"])?;

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
    pub(crate) fn wait_until_listening(&self) {
        let deadline = std::time::Instant::now() + STARTUP;

        while std::time::Instant::now() < deadline {
            if self.mix(&["status", "--no-autostart"]).status.success() {
                return;
            }

            std::thread::sleep(Duration::from_millis(25));
        }

        panic!(
            "no daemon answered on {} within {STARTUP:?}\n--- daemon.log ---\n{}",
            self.endpoint(),
            self.0.daemon_log()
        );
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        // A daemon `mix` autostarted is not this process's child and outlives the test that made
        // it. Anything still holding this home has to go before the directory does, or the next run
        // of the suite finds a daemon serving a home that no longer exists — and here rather than
        // at the end of a test body because a failed assertion is exactly when one gets left
        // behind. Nothing in here may panic — this runs while unwinding, and a panic then aborts
        // the whole run — which is why `listening_pid` swallows its failures and the stop is
        // `try_stop`: a `Daemon` dropped a moment ago may already be gone.
        if let Some(pid) = self.listening_pid() {
            // Discarded rather than asserted on: this runs while the test is already finishing, and
            // a daemon that had gone between being named and being stopped is a tidy ending rather
            // than a finding.
            let _ = try_stop(pid);
        }

        // Unconditional, and deliberately not folded into the branch above. A daemon this test is
        // *holding* has already been killed by `Daemon::drop` — locals drop in reverse declaration
        // order, so that runs first — which means `listening_pid` answers `None` on precisely the
        // runs where Windows is still letting go of the lock file and the working directory. Waiting
        // only where something answered would skip the case this constant was written for.
        std::thread::sleep(SETTLE);
    }
}

/// A daemon this test started and is responsible for.
pub(crate) struct Daemon(Child);

impl Daemon {
    /// The process it is running as.
    pub(crate) fn pid(&self) -> u32 {
        self.0.id()
    }

    /// Wait for it to end **by itself**, and say whether it did.
    ///
    /// The one assertion `mix daemon stop` cannot make from its own output: the answer arrives while
    /// the daemon is still there, on purpose, so what proves a shutdown happened is this process
    /// leaving. Polled rather than `wait`ed on, because a daemon that never goes has to fail this
    /// test rather than hang the suite — and because the kill in [`Drop`] would otherwise make every
    /// run look identical.
    pub(crate) fn wait_until_gone(&mut self) -> bool {
        let deadline = std::time::Instant::now() + mixengine_testkit::home::SHUTDOWN;

        while std::time::Instant::now() < deadline {
            if self.0.try_wait().is_ok_and(|exit| exit.is_some()) {
                return true;
            }

            std::thread::sleep(Duration::from_millis(25));
        }

        false
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Killed rather than asked to stop. `daemon.shutdown` (T9a) is what one test drives
        // deliberately; every other test wants its daemon gone whatever state it left it in, and an
        // interrupt cannot be delivered to a child portably.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The `mixengined` built alongside this `mix`.
///
/// `CARGO_BIN_EXE_…` only names binaries of the *same* package, and the daemon is another one — so
/// it is found the way `mix` itself finds it at runtime, next to the client. That is not a
/// workaround so much as the same claim under test: `cargo test --workspace`, which is what CI runs
/// and what CLAUDE.md lists, builds both into one directory.
pub(crate) fn daemon_binary() -> PathBuf {
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
///
/// **Both streams in the failure, and stdout first** — measured on a CI run that reported
/// `mix exited exit code: 1` and an empty stderr, which is exactly what a *walk* that failed looks
/// like: the plan it walked, and the service it could not reach with the reason attached, are a
/// JSON object on stdout. An assertion that shows only stderr turns that into a round trip.
pub(crate) fn json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "mix exited {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "mix --json prints JSON on stdout: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

/// What `mix` put on stdout, whether or not it succeeded.
pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// What `mix` put on stderr, which is where a question and its screen go — see `cli/src/confirm.rs`.
pub(crate) fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
