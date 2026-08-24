//! Whether this machine trusts MixEngine's own certificate authority — roadmap task **T49a**.
//!
//! T48 generated an authority and nothing that asks any operating system to believe it. This is the
//! other half: three mechanisms, one per system, none of them interchangeable — a certificate store
//! on Windows, the System keychain on macOS, and a file plus a refresh command on Linux, where which
//! file it is depends on the distribution family rather than on the platform.
//!
//! **The check is written here, pure and compiled everywhere**, exactly as [`crate::resolver`] and
//! [`crate::port_access`] are: that is what lets a developer on any one of the three test the check
//! for all three. Only the reads and the writes live in `crate::sys::trust`.
//!
//! Compiled under **both** `host` and `elevated`, for [`crate::hosts`]' reason: the daemon reads
//! whether the machine already trusts an authority and the helper is what makes it, and neither is
//! worth a second implementation.

mod check;
mod der;
// The envelope the two file-based stores are written in. Unix only, so a Windows build of the
// helper does not gain the crate — see the module header.
#[cfg(all(unix, feature = "host"))]
pub(crate) mod pem;

pub use check::{Authority, MAX_DER, Refused, is_key_id, ours, subject_of};
