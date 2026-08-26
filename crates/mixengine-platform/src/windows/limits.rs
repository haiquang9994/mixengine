//! Windows caps a service with the job object it is already in.

use crate::{
    Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl, WhenExceeded,
};

/// This machine's answer, which on Windows is every machine's answer.
#[derive(Debug, Default)]
pub(crate) struct Limits;

impl ResourceControl for Limits {
    /// **Constant, and it is allowed to be.**
    ///
    /// A job object needs nothing granted and nothing delegated: any process may create one and set
    /// limits on it. That is why this system has no [`Unavailable`](Enforcement::Unavailable) answer
    /// to give, where Linux's whole implementation is about discovering whether it has one.
    fn support(&self) -> LimitSupport {
        LimitSupport {
            mechanism: LimitMechanism::JobObject,
            cpu: Enforcement::Hard {
                when: WhenExceeded::AllocationFails,
            },
            memory: Enforcement::Hard {
                when: WhenExceeded::AllocationFails,
            },
            memory_measure: MemoryMeasure::Commit,
            priority: true,
            cores: cores(),
        }
    }
}

/// How many cores a `cpu_percent` may be spent across.
///
/// `available_parallelism` rather than a `windows-sys` call, and the reason is what this number is
/// *for*: it is the ceiling on a percentage that a person is allowed to ask for, and the standard
/// library's answer already accounts for the affinity mask this process actually has — which a raw
/// processor count would not.
///
/// **Also the divisor** in `Group::set_limits`, where a percentage of one core is turned into the
/// share of the whole machine a job object is configured with.
pub(crate) fn cores() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .try_into()
        .unwrap_or(u32::MAX)
}
