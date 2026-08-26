//! macOS has no hard cap to offer, and says so rather than pretending.

use crate::{Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl};

/// This machine's answer, which on macOS is every machine's answer.
#[derive(Debug, Default)]
pub(crate) struct Limits;

impl ResourceControl for Limits {
    /// **`Unsupported`, and it means it.**
    ///
    /// There is no per-process memory ceiling on this system and no CPU rate control — what it has
    /// is scheduling priority, which is a different promise and is reported as the one thing that is
    /// true. `.claude/features/resource-isolation.md` describes a watchdog standing in for the
    /// memory cap: warn at the threshold, restart at it on request. That is **roadmap task T71a**,
    /// and it is deliberately not here — a watchdog is a per-process RSS sample taken repeatedly,
    /// which is T71's sampler, and building a second one to serve one field on one operating system
    /// would put a loop in the supervisor that T71 would then replace.
    ///
    /// Until T71a lands, a `memory_mb` on this system is stored and not enforced, and every read of
    /// `service.limits` says so.
    fn support(&self) -> LimitSupport {
        LimitSupport {
            mechanism: LimitMechanism::None,
            cpu: Enforcement::Unsupported,
            memory: Enforcement::Unsupported,

            // Reported even though nothing here measures anything, because the field describes what
            // a number *would* mean rather than what this system does with it — and a client
            // rendering a stored `memory_mb` on macOS still has to label it.
            memory_measure: MemoryMeasure::ChargedPages,
            priority: true,
            cores: cores(),
        }
    }
}

/// How many cores a `cpu_percent` may be spent across.
///
/// The ceiling a person is allowed to ask for, which is a number this system still has an opinion
/// about even though it will not enforce anything under it.
pub(crate) fn cores() -> u32 {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .try_into()
        .unwrap_or(u32::MAX)
}
