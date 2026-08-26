//! Linux caps a service with a cgroup v2 under whatever subtree this session was delegated.

use crate::{Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl};

/// What this machine will lend of the mechanism this system has.
#[derive(Debug, Default)]
pub(crate) struct Limits;

impl ResourceControl for Limits {
    /// **A constant for now, and the next task replaces it with a probe.**
    ///
    /// Unlike the other two systems, Linux's answer is a property of the *machine* rather than of
    /// the operating system: whether a subtree is delegated at all, and which controllers were
    /// enabled inside it. Discovering that is roadmap task **T68**'s cgroup work, and until it lands
    /// this answers [`Unavailable`](Enforcement::Unavailable) — which is the honest placeholder,
    /// because it is what a machine with nothing delegated would answer anyway.
    ///
    /// [`mechanism`](LimitSupport::mechanism) is [`CgroupV2`](LimitMechanism::CgroupV2) in every
    /// case, including this one: the mechanism is what the *system* has, and `Unavailable` is what
    /// this machine will lend of it.
    fn support(&self) -> LimitSupport {
        let pending = Enforcement::Unavailable {
            why: "MixEngine has not probed this session's cgroup delegation yet".to_owned(),
        };

        LimitSupport {
            mechanism: LimitMechanism::CgroupV2,
            cpu: pending.clone(),
            memory: pending,
            memory_measure: MemoryMeasure::ChargedPages,
            priority: true,
            cores: cores(),
        }
    }
}

/// How many cores a `cpu_percent` may be spent across.
///
/// The ceiling a person is allowed to ask for. Not a divisor here, unlike its Windows counterpart:
/// `cpu.max`'s `$MAX $PERIOD` pair is per-core by construction, so `50000 100000` is half of one
/// core whatever the machine has.
pub(crate) fn cores() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .try_into()
        .unwrap_or(u32::MAX)
}
