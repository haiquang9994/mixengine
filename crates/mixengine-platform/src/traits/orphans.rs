//! What survives a daemon that was killed — roadmap task **T47a**, reporting what **T13** built.
//!
//! [ADR 0007](../../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md) settled
//! that this promise is **not the same on the three systems**, and the ADR exists to stop Windows'
//! promise being repeated where it is not true. `mix doctor` states it rather than assuming it.
//!
//! **Not a trait, and not on [`Host`](crate::Host).** It is a constant per system: a trait object
//! would suggest one machine could answer differently from another machine of the same kind, and
//! nothing about it is read off the machine at all.

/// What MixEngine can promise about a supervised service's descendants when the daemon dies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrphanGuarantee {
    /// The whole process group dies with the daemon, in the kernel.
    ///
    /// Windows: a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. The last handle closing is
    /// what kills it, which happens even when the daemon is killed with no chance to clean up.
    Total,

    /// The service itself dies; anything it started does not.
    ///
    /// Linux: `PR_SET_PDEATHSIG`. `setsid` groups but kills nothing, so a grandchild survives.
    ImmediateChild,

    /// Nothing dies on its own.
    ///
    /// macOS has neither mechanism. What covers it is the next start: the daemon adopts what
    /// survived and stops what nothing declares, which is the same path the other two run anyway.
    None,
}

impl OrphanGuarantee {
    /// What this answer means, in a sentence a person reads.
    #[must_use]
    pub fn because(self) -> &'static str {
        match self {
            Self::Total => {
                "if this daemon is killed, every process a service started dies with it — the \
                 kernel enforces it"
            }
            Self::ImmediateChild => {
                "if this daemon is killed, each service dies with it, but anything a service itself \
                 started survives until the next start adopts or stops it"
            }
            Self::None => {
                "if this daemon is killed, nothing it supervised dies on its own; the next start \
                 adopts what survived and stops what nothing declares"
            }
        }
    }
}

/// What this system guarantees.
///
/// `cfg!` and not `#[cfg]`, on [`crate::resolver`]'s reasoning one capability along: all three arms
/// compile on all three systems, so the table is checked by the compiler everywhere rather than only
/// where it happens to be true.
#[must_use]
pub fn orphan_guarantee() -> OrphanGuarantee {
    if cfg!(windows) {
        OrphanGuarantee::Total
    } else if cfg!(target_os = "linux") {
        OrphanGuarantee::ImmediateChild
    } else {
        OrphanGuarantee::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row of ADR 0007's own table that belongs to the system running this.
    #[test]
    fn this_system_makes_the_guarantee_the_adr_recorded_for_it() {
        let guarantee = orphan_guarantee();

        if cfg!(windows) {
            assert_eq!(guarantee, OrphanGuarantee::Total);
        } else if cfg!(target_os = "linux") {
            assert_eq!(guarantee, OrphanGuarantee::ImmediateChild);
        } else {
            assert_eq!(guarantee, OrphanGuarantee::None);
        }
    }

    /// Every answer has a sentence, because the sentence is the point of the check: a client renders
    /// it and a person reads it.
    #[test]
    fn every_guarantee_says_what_it_means() {
        for guarantee in [
            OrphanGuarantee::Total,
            OrphanGuarantee::ImmediateChild,
            OrphanGuarantee::None,
        ] {
            assert!(!guarantee.because().is_empty(), "{guarantee:?}");
        }
    }
}
