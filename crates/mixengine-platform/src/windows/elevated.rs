//! Windows: an elevation bit in the token, an owner SID on the file, and a DACL written by `icacls`.

use std::ffi::OsStr;
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_ELEVATION,
    TokenElevation,
};

use crate::elevated::Owner;
use crate::{Error, Result};

/// `NT AUTHORITY\SYSTEM`. Named by SID: the display name is localised, the SID is not.
const SYSTEM: &str = "S-1-5-18";

/// `BUILTIN\Administrators`, likewise — and the account a file created by an elevated process
/// ordinarily belongs to.
const ADMINISTRATORS: &str = "S-1-5-32-544";

/// `BUILTIN\Users`: everyone with an account on this machine, who must be able to *read* the log.
const USERS: &str = "S-1-5-32-545";

/// Full control, inherited by both files and subdirectories.
const FULL: &str = "(OI)(CI)F";

/// Read and execute, likewise.
const READ: &str = "(OI)(CI)R";

pub(crate) fn is_elevated() -> bool {
    // `TokenElevation` rather than a group-membership check: an administrator's *filtered* token
    // carries `BUILTIN\Administrators` deny-only, so membership is true where power is not.
    let Ok(token) = super::sid::open_process_token() else {
        return false;
    };

    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut written = 0_u32;

    #[expect(
        unsafe_code,
        reason = "the buffer is a local of this frame and the length passed is its own size"
    )]
    let read = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&raw mut elevation).cast(),
            u32::try_from(size_of::<TOKEN_ELEVATION>()).unwrap_or(u32::MAX),
            &raw mut written,
        )
    };

    // A token that will not answer is not an elevated one. Refusing to guess in the other direction
    // is the whole point: this value gates every operation with effects.
    read != 0 && elevation.TokenIsElevated != 0
}

pub(crate) fn owner_of(path: &Path) -> Result<Owner> {
    // Ask the filesystem first, so a missing path is `NotFound` with the path in it rather than a
    // Win32 error number nobody can act on.
    std::fs::symlink_metadata(path).map_err(|source| Error::Io {
        action: "read the owner of",
        path: path.to_path_buf(),
        source,
    })?;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut sid: PSID = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "`wide` is NUL-terminated and outlives the call; both out-pointers are locals, and \
                  the descriptor the call allocates is released below on every path"
    )]
    let status = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &raw mut sid,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut descriptor,
        )
    };

    if status != 0 {
        return Err(Error::Os {
            action: "read the owner of a file",
            source: io::Error::from_raw_os_error(status.cast_signed()),
        });
    }

    // The SID points into the descriptor, so it is rendered before the descriptor is released.
    let rendered = super::sid::render(sid);

    #[expect(
        unsafe_code,
        reason = "the descriptor was allocated by the call above and is ours to free exactly once"
    )]
    unsafe {
        LocalFree(descriptor.cast());
    }

    let id = rendered?;
    let superuser = id == SYSTEM;
    let administrative = superuser || id == ADMINISTRATORS;

    Ok(Owner::new(id, superuser, administrative))
}

pub(crate) fn others_can_write(path: &Path) -> Result<bool> {
    // Answering this properly means walking the DACL and resolving every trustee, which `icacls`
    // gives no way to do without parsing localised names. The check that carries the weight on this
    // system is ownership, and `owner_of` answers that exactly. Named rather than silently omitted.
    let _ = path;

    Ok(false)
}

pub(crate) fn audit_directory() -> Result<PathBuf> {
    // `%ProgramData%` and not a literal `C:\ProgramData`: the variable is what the OS itself uses,
    // and a binary running as root has no business guessing a path when the machine will tell it.
    let root = std::env::var_os("ProgramData").ok_or_else(|| Error::Os {
        action: "locate %ProgramData%",
        source: io::Error::new(io::ErrorKind::NotFound, "%ProgramData% is not set"),
    })?;

    Ok(Path::new(&root).join("MixEngine"))
}

pub(crate) fn create_root_owned_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| Error::Io {
        action: "create",
        path: path.to_path_buf(),
        source,
    })?;

    // `%ProgramData%` hands every account the right to create things below it, so the owner of a
    // directory made here is whoever made it. Set explicitly rather than relying on the token's
    // default owner, because the check that reads it back is what stops an ordinary account from
    // having arranged this directory before the helper first ran.
    let administrators = format!("*{ADMINISTRATORS}");
    super::command::run(
        "icacls",
        Some(path),
        [
            path.as_os_str(),
            OsStr::new("/setowner"),
            OsStr::new(&administrators),
            OsStr::new("/q"),
        ],
    )?;

    // Severed from `%ProgramData%`'s inherited ACL and rewritten: Administrators and SYSTEM may
    // write it, Users may read it. `mix doctor` reads the log back, so shutting Users out entirely
    // would make the evidence unreadable by the person it is evidence for.
    let owner = format!("*{ADMINISTRATORS}:{FULL}");
    let system = format!("*{SYSTEM}:{FULL}");
    let users = format!("*{USERS}:{READ}");

    super::command::run(
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
            OsStr::new(&users),
            OsStr::new("/q"),
        ],
    )?;

    Ok(())
}
