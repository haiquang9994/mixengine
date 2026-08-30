//! macOS has no hard cap to offer, and says so rather than pretending.

use crate::{Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl};

/// This machine's answer, which on macOS is every machine's answer.
#[derive(Debug, Default)]
pub(crate) struct Limits;

impl ResourceControl for Limits {
    /// **Nothing is capped here, and memory is watched** — roadmap task **T71a**.
    ///
    /// There is no per-process memory ceiling on this system and no CPU rate control. What it has is
    /// scheduling priority, which is a different promise and is reported as the one thing that is
    /// simply true.
    ///
    /// `memory` was [`Enforcement::Unsupported`] until T71a, and that was honest while nothing read
    /// the number. Something reads it now: the daemon compares a `memory_mb` against the minutes
    /// T71's sampler finishes, warns while a service is over it, and restarts the service where its
    /// recipe says a restart is safe. That is not a wall and must not be drawn as one, which is
    /// exactly what [`Enforcement::Advisory`] means.
    ///
    /// **`why: None`, and the [`None`] is the argument.** `Advisory`'s `Some` is for a machine
    /// somebody could start differently — a Linux session with no delegated `memory` controller. On
    /// macOS there is no delegation to ask for and no session to change, so a sentence here would be
    /// a line `mix doctor` printed at a person who can do nothing with it. T68's D6 said that about
    /// `Unsupported`; the reasoning outlived the variant.
    ///
    /// **`cpu` stays [`Enforcement::Unsupported`].** A rate has no watchdog equivalent: "it has been
    /// at 100% for three minutes" may be a service doing precisely what was asked of it, where "it
    /// is holding more than it was allowed" is the same fact however long it lasts.
    fn support(&self) -> LimitSupport {
        LimitSupport {
            mechanism: LimitMechanism::None,
            cpu: Enforcement::Unsupported,
            memory: Enforcement::Advisory { why: None },

            // **What the watchdog actually reads**, which is the rule the measure follows
            // everywhere: it names whatever is judging the ceiling here. RSS overstates shared
            // pages, and `MemoryMeasure::Resident` is where that is written down.
            memory_measure: MemoryMeasure::Resident,
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
