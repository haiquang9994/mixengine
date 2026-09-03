//! Linux: `/usr/local/libexec/mixengine`.

use std::path::PathBuf;

use crate::Result;

/// Root-owned on every Linux, and the same path the `.deb` and the `.rpm` write to.
///
/// **One lookup path per system, whatever put the file there** — the T85 design, D3. A distribution
/// package writing under `/usr/local` is against Debian policy and is deliberate: these packages are
/// published by us and installed by hand, and a daemon that had to look in two places depending on
/// how MixEngine arrived is a daemon with two answers to the question of which file it runs as root.
///
/// `libexec` rather than `bin` because nobody runs this by hand: it is started by the elevation
/// prompt with one argument, and by nothing else.
const HELPER: &str = "/usr/local/libexec/mixengine/mixengine-elevate";

pub(crate) fn helper_path() -> Result<PathBuf> {
    Ok(PathBuf::from(HELPER))
}

#[cfg(feature = "elevated")]
pub(crate) use crate::unix::install::make_executable;
