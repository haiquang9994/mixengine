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
