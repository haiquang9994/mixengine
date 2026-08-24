//! A private file on a system where the permission is part of creating it.

use std::fs::{OpenOptions, Permissions};
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

use crate::{Error, Result};

/// Owner read and write, and nothing else.
const OWNER_ONLY: u32 = 0o600;

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let io = |action: &'static str| {
        move |source| Error::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    };

    // **Deliberately not `.truncate(true)`.** Truncating happens below, *after* the permission is
    // settled: a file that already existed at `0644` would otherwise be emptied and refilled with
    // the key while still readable by everybody. `.mode()` applies only when this call is the one
    // that creates the file, which is why the `set_permissions` after it is not redundant with it —
    // the two cover the new file and the pre-existing one respectively, and neither covers both.
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .mode(OWNER_ONLY)
        .open(path)
        .map_err(io("create"))?;

    file.set_permissions(Permissions::from_mode(OWNER_ONLY))
        .map_err(io("restrict"))?;

    file.set_len(0).map_err(io("empty"))?;
    file.write_all(bytes).map_err(io("write"))?;

    // The caller is about to tell somebody this key exists. A write still in the page cache when
    // the machine loses power is a certificate whose key never arrived — a state the reader has a
    // name for, and one nobody should have to meet because of a missing `fsync`.
    file.sync_all().map_err(io("flush"))?;

    Ok(())
}
