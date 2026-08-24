//! macOS: the System keychain, through `/usr/bin/security`.
//!
//! **Not through Security.framework, and that is a decision about the other binary** — the T49a
//! design, D6. `SecCertificateCreateWithData`, `SecItemAdd` and `SecTrustSettingsSetTrustSettings`
//! would mean a new unsafe FFI surface inside `mixengine-elevate`, whose whole design constraint is
//! that a person can audit it by reading it, for an operation that runs once per install. The rule
//! T42 set with `pfctl` and T45 kept with `systemctl` holds here: one fixed command, a constant
//! argument vector, and no argument taken from the request.
//!
//! Reading is `security find-certificate -a -p`, which lists every certificate in a keychain as PEM
//! and **needs no administrative token** — measured by `tests/trust.rs` in CI's ordinary `test` job
//! rather than asserted here.

#[cfg(feature = "elevated")]
use crate::trust::Change;
#[cfg(feature = "host")]
use crate::{Result, TrustState, TrustStore, TrustStoreMethod};

/// The keychain a machine-wide root belongs in.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) const SYSTEM_KEYCHAIN: &str = "/Library/Keychains/System.keychain";

/// What the certificate is called while `security` reads it.
///
/// In the root-owned audit directory, so no unprivileged account can replace what is at this path
/// between the write and the read.
#[cfg(feature = "elevated")]
const HANDOFF_FILE: &str = "ca-handoff.pem";

/// Absolute, never resolved through `PATH`: this is invoked from a process holding an
/// administrative token, and a `PATH` entry is something another program can arrange.
#[cfg(any(feature = "host", feature = "elevated"))]
pub(crate) const SECURITY: &str = "/usr/bin/security";

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Trust;

#[cfg(feature = "host")]
impl TrustStore for Trust {
    fn method(&self) -> Result<TrustStoreMethod> {
        // A constant, unlike Linux: every macOS has this keychain — D7.
        Ok(TrustStoreMethod::SystemKeychain)
    }

    fn probe(&self, der: &[u8]) -> Result<TrustState> {
        let listed = certificates()?;

        // Exact DER bytes — D6. `security` also offers `-Z`, which prints SHA-1; that is a
        // different value from the SHA-256 `cert.ca_status` reports, and carrying two hashes for one
        // identity is how they come apart.
        let installed = listed.iter().any(|found| found == der);

        Ok(TrustState {
            method: TrustStoreMethod::SystemKeychain,
            installed,
            missing: (!installed).then(|| {
                format!("{SYSTEM_KEYCHAIN} does not hold MixEngine's certificate authority")
            }),
        })
    }
}

/// Every certificate in the System keychain, as DER.
///
/// Read by both directions: the install compares against it to answer `Unchanged`, and the removal
/// walks it to find what it was asked to take out.
///
/// **An empty keychain is an empty list and not an error.** `security` exits non-zero when it finds
/// nothing, which is a true answer to the question this asks and must not become a failure that
/// stops a daemon start.
///
/// **`crate::Result` written out, and not because the import is untidy.** The `Result` alias is
/// imported under `feature = "host"` and this function is compiled under `host` *or* `elevated`; in
/// the helper's build — which is the only build that ever writes a keychain — a bare `Result` is
/// `std`'s, and the mistake is a compile error nothing on Windows or Linux can reach.
#[cfg(any(feature = "host", feature = "elevated"))]
fn certificates() -> crate::Result<Vec<Vec<u8>>> {
    let output = security(
        &["find-certificate", "-a", "-p", SYSTEM_KEYCHAIN],
        "run security to read the System keychain",
    )?;

    Ok(crate::trust::pem::decode_all(&output.stdout))
}

/// How long a `security` call gets before it is treated as one that will never answer.
///
/// A read of this keychain is milliseconds and a write is not much more; thirty seconds is not a
/// budget, it is the point past which waiting has stopped being waiting.
#[cfg(any(feature = "host", feature = "elevated"))]
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long the last of the output is waited for once the command itself has exited.
#[cfg(any(feature = "host", feature = "elevated"))]
const GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `security`, with no console to ask at and a limit on how long it may take.
///
/// **Two things a helper behind an elevation prompt has to get right, and T49a had neither.**
///
/// `stdin` is `/dev/null`. `security` is a program that asks for a password when it wants one, and
/// this is called from a process that has no terminal to ask at — under `sudo` in CI, and from an
/// OS elevation prompt in front of a user who has already clicked Allow and is looking at nothing.
/// A question nobody can answer must fail, not wait.
///
/// And the wait is bounded. A privileged helper that blocks forever is worse than one that fails:
/// it holds this crate's trust lock while it does it, and the operation it was spawned for is the
/// one standing between a first run and a working machine. Whatever went wrong comes back as
/// `Failed` with the command in the message — which is a thing a person can read — rather than as
/// a job that gets cancelled twenty minutes later having printed nothing at all.
#[cfg(any(feature = "host", feature = "elevated"))]
fn security(arguments: &[&str], action: &'static str) -> crate::Result<std::process::Output> {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    let failed = |source| crate::Error::Os { action, source };

    let mut child = Command::new(SECURITY)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(failed)?;

    // **Drained on threads of their own, and that is not tidiness.** `find-certificate -a -p`
    // prints every certificate this machine trusts — a couple of hundred kilobytes against a pipe
    // buffer of 64, so a loop that polled for exit without reading would block the child on its own
    // output and then report the deadlock as a timeout.
    let reading_out = read_on_a_thread(child.stdout.take().expect("stdout was piped just above"));
    let reading_err = read_on_a_thread(child.stderr.take().expect("stderr was piped just above"));

    let deadline = Instant::now() + PATIENCE;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(failed)? {
            break status;
        }

        if Instant::now() >= deadline {
            // Killed rather than left: this process is about to exit, and a `security` still
            // waiting on something would outlive it as somebody else's child.
            let _ = child.kill();
            let _ = child.wait();

            return Err(failed(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "`security {}` did not answer within {} seconds",
                    arguments.join(" "),
                    PATIENCE.as_secs()
                ),
            )));
        }

        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    // **A grace period and not a `join`.** End of file on these pipes means every holder of the
    // write end has gone, and a grandchild the command left behind would be one — so a join here
    // would be one more unbounded wait in the same function that exists to remove them. The exit
    // status is already in hand; what arrived by now is the whole of what this can honestly report.
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
#[cfg(any(feature = "host", feature = "elevated"))]
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

/// Hand the certificate to `security` as a trusted root.
///
/// **One fixed command, and the file path is the helper's own** — the T49a design, D6. The DER
/// arrives in the request; the *path* it is written to is chosen here, so the rule T42 set with
/// `pfctl` holds: no argument comes from the request.
#[cfg(feature = "elevated")]
pub(crate) fn apply(plan: &mixengine_proto::privileged::TrustPlan) -> crate::Result<Change> {
    use mixengine_proto::privileged::TrustPlan;

    let der = match plan {
        TrustPlan::SystemKeychain { der } => der,
        TrustPlan::SystemRoot { .. }
        | TrustPlan::CaCertificates { .. }
        | TrustPlan::CaTrustAnchors { .. } => {
            return Err(crate::trust::unsupported(
                "this is macOS, whose trust store is the System keychain rather than a Windows \
                 certificate store or a Linux anchors directory",
            ));
        }
    };

    let _lock = crate::trust::held()?;

    // Read before writing, under the lock: a keychain that already holds exactly this is
    // `Unchanged`, and adding it again would raise a second trust-settings write for nothing.
    if certificates()?.iter().any(|found| found == der) {
        return Ok(Change::Unchanged);
    }

    let file = written(der)?;

    let ran = run(&[
        "add-trusted-cert",
        "-d",
        "-r",
        "trustRoot",
        "-k",
        SYSTEM_KEYCHAIN,
        &file.to_string_lossy(),
    ]);

    // The handoff file has served its purpose whether or not `security` accepted it, and leaving a
    // certificate lying in a root-owned directory is litter the next run would read.
    let _ = std::fs::remove_file(&file);
    ran?;

    Ok(Change::Written {
        detail: format!("added MixEngine's certificate authority to {SYSTEM_KEYCHAIN}"),
    })
}

/// Take it back out, having first checked that what is there is ours.
#[cfg(feature = "elevated")]
pub(crate) fn revoke(target: &mixengine_proto::privileged::TrustTarget) -> crate::Result<Change> {
    use mixengine_proto::privileged::TrustTarget;

    let key_id = match target {
        TrustTarget::SystemKeychain { key_id } => key_id,
        TrustTarget::SystemRoot { .. }
        | TrustTarget::CaCertificates { .. }
        | TrustTarget::CaTrustAnchors { .. } => {
            return Err(crate::trust::unsupported(
                "this is macOS, whose trust store is the System keychain",
            ));
        }
    };

    let _lock = crate::trust::held()?;

    // **D5's second check.** Every certificate in the keychain that both passes the shape check and
    // carries the authority that was named — nothing else is touched, and a keychain holding a
    // corporate root is a keychain this cannot be aimed at.
    let mut removed = 0;
    for der in certificates()? {
        let ours = crate::trust::ours(&der).is_ok_and(|authority| &authority.key_id == key_id);
        if !ours {
            continue;
        }

        let file = written(&der)?;
        let ran = run(&["remove-trusted-cert", "-d", &file.to_string_lossy()]);
        let _ = std::fs::remove_file(&file);
        ran?;
        removed += 1;
    }

    if removed == 0 {
        return Ok(Change::Unchanged);
    }

    Ok(Change::Written {
        detail: format!(
            "removed MixEngine's certificate authority {key_id} from {SYSTEM_KEYCHAIN}"
        ),
    })
}

/// The certificate in a file whose name this process chose and whose directory only root can write.
///
/// **`tempfile` is a dev-dependency and stays one.** `security` needs a path, and the two ways to
/// give it one are a crate in a binary that runs as root or a directory this project already owns.
/// The audit directory is root-owned, already exists — the locks live in it — and gives the file a
/// fixed name, so nothing about this path comes from a request and no unprivileged account can
/// swap what is at it between the write and the read.
#[cfg(feature = "elevated")]
fn written(der: &[u8]) -> crate::Result<std::path::PathBuf> {
    let path = crate::elevated::audit_directory()?.join(HANDOFF_FILE);

    std::fs::write(&path, crate::trust::pem::encode(der)).map_err(|source| crate::Error::Io {
        action: "write the certificate for this machine's keychain",
        path: path.clone(),
        source,
    })?;

    Ok(path)
}

/// Run `security` with a fixed verb and this process's own file path.
#[cfg(feature = "elevated")]
fn run(arguments: &[&str]) -> crate::Result<()> {
    let output = security(arguments, "run security to change the System keychain")?;

    if output.status.success() {
        return Ok(());
    }

    // The verb as well as the complaint. `security` says "The specified item could not be found in
    // the keychain" for several different requests, and which one was made is the half of that
    // sentence a person needs.
    Err(crate::Error::Os {
        action: "change this machine's System keychain",
        source: std::io::Error::other(format!(
            "`security {}` failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    })
}
