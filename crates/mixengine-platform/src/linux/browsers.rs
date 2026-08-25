//! Linux: `certutil`, and the databases Firefox and Chrome read instead of the system store.
//!
//! **The tool is not installed on a stock Ubuntu 24.04** — the T49b design, D7, and the measurement
//! that task started from. It ships in `libnss3-tools`, which `libnss3` does not pull in. A machine
//! without it is a state to report, and the report names the package.
//!
//! **Every invocation gets a null stdin and a deadline** — D8, which is T49a's macOS lesson applied
//! before it cost anything here. `security remove-trusted-cert` once waited twenty minutes in CI
//! printing nothing, because `Command::output()` inherits stdin and has no timeout; `certutil` has
//! the same failure available to it, since a Firefox profile with a master password set makes it
//! prompt. A daemon start that inherited stdin would never finish.
//!
//! **One database's failure never fails the survey.** Each is independent, and a locked or broken
//! profile is a line in the report rather than an error return.

use std::path::Path;
use std::time::{Duration, Instant};

use crate::{BrowserSurvey, BrowserTrust, DatabaseState, Result};

/// Resolved through `PATH` rather than named absolutely, unlike macOS's `/usr/bin/security`: this
/// runs as the ordinary user in that user's own home, so a `PATH` entry here is that user's own
/// choice rather than something another account arranged for a process holding a token.
const CERTUTIL: &str = "certutil";

/// What a machine without the tool needs, named so nobody has to search for it.
const PACKAGE: &str = "libnss3-tools";

/// How long one `certutil` may take before it is killed — D8.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long the last of the output is waited for once the command itself has exited.
const GRACE: Duration = Duration::from_secs(5);

/// This system's answer.
///
/// **Carries the home it searches rather than asking for one per call.** [`HomeDirs`](crate::HomeDirs)
/// answers where *MixEngine's* data goes; these databases are the user's own, so this is the other
/// question — and resolving it once, into a field, is what lets `tests/browsers.rs` point the whole
/// search at a temp directory. That redirection is the entire isolation
/// `.claude/standards/testing.md`'s first rule needs here: nothing ever goes near a real profile.
#[derive(Debug)]
pub(crate) struct Browsers {
    home: std::path::PathBuf,
}

impl Browsers {
    /// The real one.
    pub(crate) fn of_this_user() -> Self {
        Self {
            home: directories::BaseDirs::new()
                .map(|base| base.home_dir().to_path_buf())
                .unwrap_or_default(),
        }
    }

    /// One rooted anywhere, for tests. An empty home finds nothing, which is also the honest answer
    /// for a machine whose home directory could not be resolved at all.
    #[cfg(test)]
    pub(crate) fn under(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }
}

impl BrowserTrust for Browsers {
    fn survey(&self, der: &[u8]) -> Result<BrowserSurvey> {
        if !available() {
            return Ok(BrowserSurvey::NoTool {
                because: format!(
                    "{CERTUTIL} is not installed, so Firefox and Chrome were not asked — it ships \
                     in {PACKAGE}"
                ),
            });
        }

        let databases = crate::browsers::databases_under(&self.home)
            .into_iter()
            .map(|database| state(&database, der))
            .collect();

        Ok(BrowserSurvey::Reached { databases })
    }
}

/// Is the tool on this machine at all?
///
/// **Run rather than looked for on `PATH`.** A file called `certutil` is not proof it is NSS's — the
/// name collides with CryptoAPI's on Windows, which is one of the reasons D2 does not search there
/// — and the cheapest honest test is whether it answers.
fn available() -> bool {
    // `certutil --help` exits non-zero on some builds while still being the right program, so what
    // is tested is that it ran and said something, never its status.
    certutil(&["--help"]).is_ok_and(|output| !output.stdout.is_empty() || !output.stderr.is_empty())
}

/// What one database says about `der`.
///
/// **Never an error**: a profile that could not be read is one line in the report, and every other
/// database is still answered.
fn state(database: &crate::browsers::Database, der: &[u8]) -> DatabaseState {
    let path = database.directory.display().to_string();
    let owner = database.owner.to_owned();

    let Ok(authority) = crate::trust::ours(der) else {
        return DatabaseState {
            path,
            owner,
            installed: false,
            because: Some(
                "these bytes are not an authority MixEngine made, so no database was asked about \
                 them"
                    .to_owned(),
            ),
        };
    };

    let nickname = crate::trust::subject_of(&authority.key_id);

    match read(&database.directory, &nickname) {
        Ok(found) => {
            // Exact DER bytes. A nickname match alone would claim another home's authority as this
            // one's, which is the comparison every probe T49a wrote makes.
            let installed = found.as_deref() == Some(der);

            DatabaseState {
                path: path.clone(),
                owner,
                installed,
                because: (!installed).then(|| format!("{path} does not hold this authority")),
            }
        }
        Err(error) => DatabaseState {
            path,
            owner,
            installed: false,
            because: Some(mixengine_proto::flatten(&error)),
        },
    }
}

/// What sits under `nickname` in this database, as DER.
fn read(directory: &Path, nickname: &str) -> Result<Option<Vec<u8>>> {
    let output = certutil(&["-L", "-d", &sql(directory), "-n", nickname, "-a"])?;

    if !output.status.success() {
        // A nickname that is not there is `certutil`'s ordinary "not found" and not a fault: an
        // empty database is the state of most machines and must not read as an error.
        return Ok(None);
    }

    Ok(listed(&output.stdout))
}

/// The DER inside what `certutil -L -a` printed, or [`None`] when it printed no certificate.
fn listed(text: &[u8]) -> Option<Vec<u8>> {
    crate::trust::pem::decode(text)
}

/// How `certutil` is told which database, and the only format this build addresses — D5.
fn sql(directory: &Path) -> String {
    format!("sql:{}", directory.display())
}

/// Run `certutil`, with no console to ask at and a limit on how long it may take — D8.
///
/// The shape is `macos/trust.rs`'s `security` helper, ported rather than shared: the two live under
/// different `cfg`s, and a shared helper would have to be compiled on both.
fn certutil(arguments: &[&str]) -> Result<std::process::Output> {
    use std::process::{Command, Stdio};

    let action = "run certutil";
    let failed = |source| crate::Error::Os { action, source };

    let mut child = Command::new(CERTUTIL)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(failed)?;

    // **Drained on threads of their own, and that is not tidiness.** A listing is larger than a
    // pipe buffer, so a loop that polled for exit without reading would block the child on its own
    // output and then report the deadlock as a timeout.
    let reading_out = read_on_a_thread(child.stdout.take().expect("stdout was piped just above"));
    let reading_err = read_on_a_thread(child.stderr.take().expect("stderr was piped just above"));

    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(failed)? {
            break status;
        }

        if Instant::now() >= deadline {
            // Killed rather than left: a `certutil` still waiting on a password prompt would
            // otherwise outlive this daemon as somebody else's child.
            let _ = child.kill();
            let _ = child.wait();

            return Err(failed(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "`{CERTUTIL} {}` did not answer within {} seconds, which is what a profile \
                     asking for a master password looks like",
                    arguments.join(" "),
                    PATIENCE.as_secs()
                ),
            )));
        }

        std::thread::sleep(Duration::from_millis(20));
    };

    // **A grace period and not a `join`.** End of file on these pipes means every holder of the
    // write end has gone, and a grandchild the command left behind would be one — so a join here
    // would be one more unbounded wait in the function that exists to remove them.
    Ok(std::process::Output {
        status,
        stdout: reading_out.recv_timeout(GRACE).unwrap_or_default(),
        stderr: reading_err.recv_timeout(GRACE).unwrap_or_default(),
    })
}

/// Read one pipe to the end on a thread, and hand back whatever arrived.
///
/// A read error ends the thread with what it has rather than being raised: what the caller needs is
/// an exit status, and the bytes that did arrive are still the program's account of itself.
fn read_on_a_thread<R: std::io::Read + Send + 'static>(
    mut pipe: R,
) -> std::sync::mpsc::Receiver<Vec<u8>> {
    let (finished, done) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = finished.send(bytes);
    });

    done
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `certutil -L -n <nickname> -a` prints for a certificate it holds.
    fn listing(der: &[u8]) -> String {
        crate::trust::pem::encode(der)
    }

    /// The bytes come back through the envelope, which is the only property the probe rests on.
    #[test]
    fn a_listing_is_read_back_as_the_certificate_that_was_written() {
        let der = vec![0x30, 0x82, 0x01, 0x00];

        assert_eq!(listed(listing(&der).as_bytes()), Some(der));
    }

    /// `certutil` prints a diagnostic and no envelope when the nickname is not there; that is not a
    /// certificate and must not read as one.
    #[test]
    fn a_database_without_the_nickname_reads_as_nothing() {
        assert_eq!(
            listed(b"certutil: Could not find cert: MixEngine Local CA 0123abcd\n"),
            None
        );
        assert_eq!(listed(b""), None);
    }

    /// The name is built in one place and derived from the authority — never a second `format!`,
    /// which is how a removal comes to look for a name an install never wrote.
    #[test]
    fn the_nickname_is_the_subject_t48_writes() {
        assert_eq!(
            crate::trust::subject_of("0123abcd"),
            "MixEngine Local CA 0123abcd"
        );
    }

    /// A home with no databases is answered, not searched twice: `survey` reaches the discovery in
    /// `crate::browsers` and nothing else.
    #[test]
    fn a_home_with_no_databases_is_reached_and_empty() {
        let home = tempfile::tempdir().expect("a temp home");
        let browsers = Browsers::under(home.path());

        // Only meaningful on a machine that has the tool; one without it answers `NoTool`, which is
        // the other branch and is asserted by its own reason rather than by this fixture.
        match browsers.survey(&[1, 2, 3]).expect("the survey answers") {
            BrowserSurvey::Reached { databases } => assert!(databases.is_empty()),
            BrowserSurvey::NoTool { because } => assert!(because.contains(PACKAGE)),
            other => panic!("a Linux host answered {other:?}"),
        }
    }
}
