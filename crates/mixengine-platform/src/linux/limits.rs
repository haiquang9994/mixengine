//! Linux caps a service with a cgroup v2 under whatever subtree this session was delegated.

use crate::{
    Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl, WhenExceeded,
};

/// What this machine will lend of the mechanism this system has.
#[derive(Debug, Default)]
pub(crate) struct Limits;

impl ResourceControl for Limits {
    /// **Measured, not assumed, and measured per controller.**
    ///
    /// Unlike the other two systems, Linux's answer is a property of the *machine* rather than of
    /// the operating system: whether a subtree is delegated at all, and which controllers were
    /// enabled inside it. Both are read here, at every call — the probe is two small file operations
    /// and the answer can change under a running daemon when a session is reconfigured.
    ///
    /// [`mechanism`](LimitSupport::mechanism) is [`CgroupV2`](LimitMechanism::CgroupV2) in every
    /// case, including the ones that enforce nothing: the mechanism is what the *system* has, and
    /// [`Unavailable`](Enforcement::Unavailable) is what this machine will lend of it. Reporting
    /// [`Unsupported`](Enforcement::Unsupported) here would say the wrong thing — that no release
    /// and no reconfiguration could ever make this work.
    fn support(&self) -> LimitSupport {
        let (cpu, memory) = match super::cgroup::Delegation::discover() {
            Ok(delegation) => {
                let controllers = delegation.controllers();

                (
                    cpu_enforcement(controllers.cpu),
                    memory_enforcement(controllers.memory),
                )
            }

            // No delegated subtree at all: one reason, and it is the same reason for both fields —
            // though not the same *answer*, since T71a reads a memory ceiling this machine will not
            // hold and does nothing about a CPU one.
            Err(why) => (
                Enforcement::Unavailable { why: why.clone() },
                Enforcement::Advisory { why: Some(why) },
            ),
        };

        // **The measure names whatever is judging the ceiling**, so it follows the answer above
        // rather than being a constant: the kernel charges pages where it is capping, and the
        // daemon's watchdog reads RSS where it is not.
        let memory_measure = if matches!(memory, Enforcement::Hard { .. }) {
            MemoryMeasure::ChargedPages
        } else {
            MemoryMeasure::Resident
        };

        LimitSupport {
            mechanism: LimitMechanism::CgroupV2,
            cpu,
            memory,
            memory_measure,
            priority: true,
            cores: cores(),
        }
    }
}

/// The `cpu` controller's answer, turned into the word a client reads.
///
/// [`Killed`](WhenExceeded::Killed) is the honest summary of what a person eventually sees, even
/// though reclaiming is what the kernel tries first — see `cgroup.rs`, where `memory.high` is set
/// equal to `memory.max` precisely so that reclaiming gets its chance.
fn cpu_enforcement(controller: std::result::Result<(), String>) -> Enforcement {
    controller.map_or_else(
        |why| Enforcement::Unavailable { why },
        |()| Enforcement::Hard {
            when: WhenExceeded::Killed,
        },
    )
}

/// The `memory` controller's answer — roadmap task **T71a**.
///
/// **`Advisory` where `cpu` would say `Unavailable`, and the difference is that something now
/// happens.** A session with no delegated `memory` controller used to store a `memory_mb` and do
/// nothing with it. The daemon's watchdog reads it: over the line for long enough, the service is
/// warned about and, where its recipe permits, restarted.
///
/// **The sentence is kept.** This is still a machine somebody could start differently, which is
/// exactly what `Advisory`'s [`Some`] means and what separates it from macOS's [`None`] — and it is
/// still the line `mix doctor` prints.
fn memory_enforcement(controller: std::result::Result<(), String>) -> Enforcement {
    controller.map_or_else(
        |why| Enforcement::Advisory { why: Some(why) },
        |()| Enforcement::Hard {
            when: WhenExceeded::Killed,
        },
    )
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
