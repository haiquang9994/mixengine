//! Linux: `cap_net_bind_service`, read straight off the file.
//!
//! **`getxattr` and not `getcap`** — the T42 design, D8: `libcap` is a package that may not be
//! installed, and this runs on every daemon start on every Linux machine. It was measured that an
//! ordinary user can read the attribute back in full, which is what makes probing on every start
//! cost nothing.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

#[cfg(feature = "elevated")]
use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget};

use crate::port_access::capability;
use crate::{Error, Result};

#[cfg(feature = "host")]
use crate::{PortAccess, PortAccessMethod, PortAccessState, PortBinding};

/// The first port an ordinary account may bind.
#[cfg(feature = "host")]
const FIRST_UNRESERVED: u16 = 1024;

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Ports;

#[cfg(feature = "host")]
impl PortAccess for Ports {
    /// A capability lets the program bind the reserved port itself, so the two numbers are the same
    /// one on this system.
    fn bindings(&self, answering: &[u16]) -> Vec<PortBinding> {
        answering
            .iter()
            .map(|&answer| PortBinding {
                answer,
                bind: answer,
            })
            .collect()
    }

    fn probe(&self, binary: &Path, answering: &[u16]) -> Result<PortAccessState> {
        let bindings = self.bindings(answering);

        if answering.iter().all(|port| *port >= FIRST_UNRESERVED) {
            return Ok(PortAccessState {
                method: PortAccessMethod::Capability,
                bindings,
                granted: true,
                missing: None,
            });
        }

        let granted = read(binary)?
            .as_deref()
            .is_some_and(capability::grants_bind);

        Ok(PortAccessState {
            method: PortAccessMethod::Capability,
            bindings,
            granted,
            missing: (!granted).then(|| {
                format!(
                    "{} does not hold cap_net_bind_service; any write to the file clears it, so an \
                     update is the usual reason",
                    binary.display()
                )
            }),
        })
    }
}

/// The `security.capability` attribute of `path`, or [`None`] when it has none.
///
/// # Errors
///
/// [`Error::Io`] for anything other than "no such attribute" and "this filesystem does not carry
/// them", both of which are the same answer as no capability.
#[expect(
    unsafe_code,
    reason = "std cannot read an extended attribute, and libcap is a package a machine may not have"
)]
pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let (file, name) = names(path)?;

    // A revision-3 `vfs_cap_data` is 24 bytes. 64 is room for a format nobody has published yet,
    // and it is a stack buffer, so the slack costs nothing.
    let mut buffer = [0u8; 64];

    // SAFETY: `file` and `name` are NUL-terminated C strings that outlive the call and are read and
    // not written. `buffer` is `buffer.len()` bytes of this stack frame; `getxattr` writes at most
    // that many and returns how many it wrote, or -1.
    let read = unsafe {
        libc::getxattr(
            file.as_ptr(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };

    if read >= 0 {
        let read = usize::try_from(read).unwrap_or(0);
        return Ok(Some(buffer[..read].to_vec()));
    }

    let source = std::io::Error::last_os_error();

    match source.raw_os_error() {
        // No attribute at all, and a filesystem that carries none. Both are "nothing is granted",
        // which is exactly what this was asked.
        Some(libc::ENODATA | libc::ENOTSUP) => Ok(None),
        _ => Err(Error::Io {
            action: "read the capabilities of",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The two C strings every one of these three calls needs.
fn names(path: &Path) -> Result<(CString, CString)> {
    let nul = |_| Error::Io {
        action: "name",
        path: path.to_path_buf(),
        source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "a path holding a NUL"),
    };

    Ok((
        CString::new(path.as_os_str().as_bytes()).map_err(nul)?,
        CString::new(capability::XATTR).map_err(nul)?,
    ))
}

/// Grant `cap_net_bind_service` — the T42 design, D8 and D11.
///
/// **What bounds this is the kernel, and it was measured rather than assumed**: any write to the
/// file clears the attribute, by `cp` and by `mv` alike. So what root approved is the exact bytes
/// that were there when it approved them, and substituted code arrives with no capability and has to
/// ask again.
///
/// # Errors
///
/// [`Error::UnsupportedPlatform`] for a redirect, which is not this system's mechanism, and
/// [`Error::Io`] when the attribute cannot be written.
#[cfg(feature = "elevated")]
pub(crate) fn apply(plan: &PortAccessPlan) -> Result<crate::port_access::Change> {
    let PortAccessPlan::Capability { binary, .. } = plan else {
        return Err(Error::UnsupportedPlatform {
            capability: "PortAccess",
            reason: "Linux grants a capability on the binary and has no packet-filter redirect to \
                     install; nothing was changed"
                .to_owned(),
        });
    };

    let _held = crate::port_access::held()?;

    if read(binary)?
        .as_deref()
        .is_some_and(capability::grants_bind)
    {
        return Ok(crate::port_access::Change::Unchanged);
    }

    write(binary)?;

    Ok(crate::port_access::Change::Written {
        detail: format!("granted cap_net_bind_service to {}", binary.display()),
    })
}

/// Take it off again.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub(crate) fn revoke(target: &PortAccessTarget) -> Result<crate::port_access::Change> {
    let PortAccessTarget::Capability { binary } = target else {
        return Err(Error::UnsupportedPlatform {
            capability: "PortAccess",
            reason: "Linux has no packet-filter redirect to remove; nothing was changed".to_owned(),
        });
    };

    let _held = crate::port_access::held()?;

    if read(binary)?.is_none() {
        return Ok(crate::port_access::Change::Unchanged);
    }

    clear(binary)?;

    Ok(crate::port_access::Change::Written {
        detail: format!("took cap_net_bind_service off {}", binary.display()),
    })
}

/// Write the attribute.
#[cfg(feature = "elevated")]
#[expect(
    unsafe_code,
    reason = "std cannot write an extended attribute, and libcap is a package a machine may not have"
)]
fn write(path: &Path) -> Result<()> {
    let (file, name) = names(path)?;

    // SAFETY: `file` and `name` are NUL-terminated C strings that outlive the call and are read and
    // not written. `ENCODED` is a 20-byte constant and its length is passed as such. Flags 0 means
    // "create or replace", which is what a whole-state operation wants.
    let written = unsafe {
        libc::setxattr(
            file.as_ptr(),
            name.as_ptr(),
            capability::ENCODED.as_ptr().cast(),
            capability::ENCODED.len(),
            0,
        )
    };

    if written == 0 {
        Ok(())
    } else {
        Err(Error::Io {
            action: "grant a capability on",
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        })
    }
}

/// Remove the attribute.
#[cfg(feature = "elevated")]
#[expect(
    unsafe_code,
    reason = "std cannot remove an extended attribute; this is `write`'s reverse and its pair"
)]
fn clear(path: &Path) -> Result<()> {
    let (file, name) = names(path)?;

    // SAFETY: as `write` above — two NUL-terminated C strings that outlive the call and are read
    // and not written.
    let removed = unsafe { libc::removexattr(file.as_ptr(), name.as_ptr()) };

    if removed == 0 {
        return Ok(());
    }

    let source = std::io::Error::last_os_error();

    // Somebody else got there first, which is the state this was asked for.
    if source.raw_os_error() == Some(libc::ENODATA) {
        return Ok(());
    }

    Err(Error::Io {
        action: "take a capability off",
        path: path.to_path_buf(),
        source,
    })
}
