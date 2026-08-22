//! Where Windows keeps the hosts file, and how a file is replaced there.

use std::path::PathBuf;

/// `%SystemRoot%\System32\drivers\etc\hosts`, with the directory read from the environment.
///
/// **Not hard-coded to `C:\Windows`**: a machine imaged onto another drive letter is unusual and is
/// not something a binary running as an administrator should be guessing about. `SystemRoot` is set
/// by the kernel on every Windows process, so the fallback below is unreachable in practice and is
/// written rather than unwrapped because nothing in this crate panics.
pub(crate) fn path() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());

    PathBuf::from(root)
        .join("System32")
        .join("drivers")
        .join("etc")
        .join("hosts")
}
