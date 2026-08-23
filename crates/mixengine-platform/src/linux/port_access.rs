//! Linux: `cap_net_bind_service`, read straight off the file.
//!
//! **`getxattr` and not `getcap`** — the T42 design, D8: `libcap` is a package that may not be
//! installed, and this runs on every daemon start on every Linux machine. It was measured that an
//! ordinary user can read the attribute back in full, which is what makes probing on every start
//! cost nothing.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

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
    fn probe(&self, binary: &Path, answering: &[u16]) -> Result<PortAccessState> {
        // A capability lets the program bind the reserved port itself, so the two numbers are the
        // same one on this system.
        let bindings = answering
            .iter()
            .map(|&answer| PortBinding {
                answer,
                bind: answer,
            })
            .collect();

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
