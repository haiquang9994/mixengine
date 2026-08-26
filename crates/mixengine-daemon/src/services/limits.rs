//! Turning a declared `ResourceLimits` into the value the spawn layer speaks.
//!
//! **Two types for one idea, and the seam is deliberate.** `mixengine-platform`'s `process` module
//! is compiled by `mixengine-shim` without `mixengine-proto` — the `process` feature does not enable
//! that dependency, only `host` and `elevated` do — so it cannot name the proto type without adding
//! proto to the shim's dependency closure to describe a value the shim never has. The T68 design,
//! D9, and the same argument `mixengine_platform::process::Signal` already carries.
//!
//! This function is the whole of what that costs, and this module exists so that it is the whole:
//! nothing else in the daemon converts between the two.

use mixengine_platform::process::{Limits, Priority};
use mixengine_proto::ResourceLimits;

/// What the spawn layer should apply for this declared set of limits.
pub(crate) const fn from_proto(declared: ResourceLimits) -> Limits {
    Limits {
        cpu_percent: declared.cpu_percent,
        memory_mb: declared.memory_mb,
        priority: match declared.priority {
            mixengine_proto::Priority::Normal => Priority::Normal,
            mixengine_proto::Priority::Background => Priority::Background,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field crosses, and the enum crosses as itself rather than as a number.
    #[test]
    fn a_declared_limit_becomes_the_one_the_spawn_layer_applies() {
        let converted = from_proto(ResourceLimits {
            cpu_percent: Some(50),
            memory_mb: Some(512),
            priority: mixengine_proto::Priority::Background,
        });

        assert_eq!(converted.cpu_percent, Some(50));
        assert_eq!(converted.memory_mb, Some(512));
        assert_eq!(converted.priority, Priority::Background);
    }

    /// The ordinary service, which is every service until somebody caps one.
    #[test]
    fn an_uncapped_service_converts_to_an_uncapped_spawn() {
        assert_eq!(from_proto(ResourceLimits::default()), Limits::default());
    }
}
