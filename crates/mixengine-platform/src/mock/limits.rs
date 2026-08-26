//! The mock's limit support — whatever a test said.

use crate::{
    Enforcement, LimitMechanism, LimitSupport, MemoryMeasure, ResourceControl, WhenExceeded,
};

/// What this mock will answer.
#[derive(Debug)]
pub(crate) struct Limits {
    /// The answer a test configured, or [`Default`]'s.
    pub(crate) support: LimitSupport,
}

impl Default for Limits {
    /// **A machine that can cap things, by default.**
    ///
    /// So that the ordinary test does not have to configure one. The degraded answers are the
    /// interesting ones — a controller this session was not given, a field this system will never
    /// support — and are therefore the ones a test sets deliberately, through
    /// [`Host::set_limit_support`](super::Host::set_limit_support).
    fn default() -> Self {
        Self {
            support: LimitSupport {
                mechanism: LimitMechanism::CgroupV2,
                cpu: Enforcement::Hard {
                    when: WhenExceeded::Killed,
                },
                memory: Enforcement::Hard {
                    when: WhenExceeded::Killed,
                },
                memory_measure: MemoryMeasure::ChargedPages,
                priority: true,
                cores: 4,
            },
        }
    }
}

impl ResourceControl for Limits {
    fn support(&self) -> LimitSupport {
        self.support.clone()
    }
}
