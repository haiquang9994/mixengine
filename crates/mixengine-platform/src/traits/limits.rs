//! Asking this machine what it will enforce of a service's declared limits.
//!
//! The vocabulary is `mixengine-proto`'s — see [`mixengine_proto::limits`] — because these values
//! cross the API to a client. What is here is the question.

pub use mixengine_proto::{Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, WhenExceeded};

/// What this machine will enforce of a service's declared limits.
///
/// **Reads only.** Applying a limit is not a question asked of the machine: it happens to a
/// particular child, through the handle that spawned it, at the moment it is spawned or while it
/// runs — and that lives in [`process`](crate::process). The same split
/// [`PortAccess`](crate::PortAccess) makes, for the same reason.
pub trait ResourceControl: std::fmt::Debug + Send + Sync {
    /// What this machine will enforce, field by field.
    fn support(&self) -> LimitSupport;
}
