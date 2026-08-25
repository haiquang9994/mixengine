//! The certificate databases Firefox and Chrome read instead of the system store — roadmap task
//! **T49b**.
//!
//! **Discovery is written here, pure and compiled everywhere**, exactly as [`crate::trust`]'s check
//! is and for its reason: that is what lets a developer on any one of the three systems test it for
//! Linux. Only the `certutil` invocations live in this system's own module, and only Linux has any.
//!
//! `host` only. Nothing here needs privilege — these databases belong to the user, which is the
//! line T49 was split on — so `mixengine-elevate` gains no line from this module.

mod roots;

pub use roots::{Database, databases_under};

/// A view of the databases under `home` rather than under this user's own.
///
/// **For `tests/browsers.rs`, and `#[doc(hidden)]` because it is not a capability.** That suite
/// makes a database in a temp directory with `certutil -N`, and pointing the whole search at that
/// directory is what keeps `.claude/standards/testing.md`'s first rule: nothing it writes into is a
/// store anybody's browser reads.
///
/// Linux only, because it is the only system with an implementation to point anywhere.
#[cfg(target_os = "linux")]
#[doc(hidden)]
#[must_use]
pub fn under(home: &std::path::Path) -> impl crate::BrowserTrust {
    crate::sys::browsers::Browsers::under(home)
}
