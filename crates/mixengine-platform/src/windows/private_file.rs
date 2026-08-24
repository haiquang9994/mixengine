//! A private file on a system where the ACL can only be written after the file exists.
//!
//! **It cannot call [`restrict_to_owner`](crate::DirectoryAccess::restrict_to_owner).** That grants
//! `(OI)(CI)F` — Object Inherit and Container Inherit, which describe what a *directory* hands down
//! to what is created inside it — and `icacls` refuses both flags on a file. The three accounts are
//! the same three; the grant is not, and `access.rs` is where the shape of one is decided.

use std::ffi::OsStr;
use std::path::Path;

use super::access::{Access, matches_what_we_apply};
use super::command::run;
use crate::{Error, Result};

/// `NT AUTHORITY\SYSTEM`. Named by SID: the display name is localised, the SID is not.
const SYSTEM: &str = "S-1-5-18";

/// `BUILTIN\Administrators`, likewise.
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// Full control over this one file. **No `(OI)(CI)`** — see the module documentation.
const FULL: &str = "F";

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let io = |action: &'static str| {
        move |source| Error::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    };

    // **Empty first, and that ordering is the whole of what this function is for.** An ACL cannot
    // name a file that does not exist, so the content must not exist before the ACL does. This is
    // the same window the Unix half closes with `open(2)`'s mode argument, closed here by there
    // being nothing worth reading inside it. Truncating an existing file at the same moment is
    // deliberate: a longer key left by an older version does not survive past this line.
    std::fs::write(path, b"").map_err(io("create"))?;

    restrict(path)?;

    std::fs::write(path, bytes).map_err(io("write"))
}

pub(crate) fn is_private(path: &Path) -> Result<bool> {
    // `icacls` does report a missing path, but as a failure indistinguishable from every other
    // failure. Ask the filesystem first, so the caller gets a named `NotFound` rather than a parse
    // of an English error string — `is_restricted_to_owner`'s reasoning, one path along.
    std::fs::metadata(path).map_err(|source| Error::Io {
        action: "read the permissions of",
        path: path.to_path_buf(),
        source,
    })?;

    let listing = run("icacls", Some(path), [path.as_os_str()])?;

    Ok(matches_what_we_apply(&listing))
}

/// Sever what this file inherited, and grant the three accounts that may read it.
fn restrict(path: &Path) -> Result<()> {
    let access = Access::default();
    let sid = access.current_user_sid()?;

    let owner = format!("*{sid}:{FULL}");
    let system = format!("*{SYSTEM}:{FULL}");
    let administrators = format!("*{ADMINISTRATORS}:{FULL}");

    // `/reset` first, for the reason `access.rs` gives at length: `icacls` rejects it in company
    // with `/inheritance:r`, and without it an ACE that is *explicit* rather than inherited
    // survives everything below and keeps granting whoever it granted. A key file restored from a
    // backup carrying its own ACL is exactly that case.
    run(
        "icacls",
        Some(path),
        [path.as_os_str(), OsStr::new("/reset"), OsStr::new("/q")],
    )?;

    run(
        "icacls",
        Some(path),
        [
            path.as_os_str(),
            OsStr::new("/inheritance:r"),
            OsStr::new("/grant:r"),
            OsStr::new(&owner),
            OsStr::new("/grant:r"),
            OsStr::new(&system),
            OsStr::new("/grant:r"),
            OsStr::new(&administrators),
            // Say nothing on success: this runs once per key written, and a line per file is not
            // what a daemon's log is for.
            OsStr::new("/q"),
        ],
    )?;

    Ok(())
}
