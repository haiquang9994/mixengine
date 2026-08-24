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
#[cfg(any(feature = "host", feature = "elevated"))]
fn certificates() -> Result<Vec<Vec<u8>>> {
    let output = std::process::Command::new(SECURITY)
        .args(["find-certificate", "-a", "-p", SYSTEM_KEYCHAIN])
        .output()
        .map_err(|source| crate::Error::Os {
            action: "run security to read the System keychain",
            source,
        })?;

    Ok(crate::trust::pem::decode_all(&output.stdout))
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
    let output = std::process::Command::new(SECURITY)
        .args(arguments)
        .output()
        .map_err(|source| crate::Error::Os {
            action: "run security to change the System keychain",
            source,
        })?;

    if output.status.success() {
        return Ok(());
    }

    Err(crate::Error::Os {
        action: "change this machine's System keychain",
        source: std::io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_owned()),
    })
}
