//! What this machine's application control policy is doing about unsigned images — roadmap task
//! **T94**.
//!
//! **Two claims, kept apart on purpose** (T94 design, D3). [`AppControl`] reads a value that
//! describes **Smart App Control** and nothing else. [`refused_by_app_control`] recognises a refusal
//! that an *enterprise WDAC policy* produces just as readily, on a machine whose Smart App Control
//! is off — so it says "an application control policy" and never names the feature. Collapsing the
//! two would send somebody on a corporate laptop to a setting that is not the one refusing them.
//!
//! **The half that is a decision is compiled everywhere.** [`AppControlState::from_policy_value`]
//! and [`refused_by_app_control`] are pure and are tested on all three systems, which is the rule
//! [`crate::reserved`](crate) follows; only the registry call sits behind a `cfg`.

use crate::Result;

/// The OS error a Windows image load refused by Code Integrity arrives as.
///
/// **Measured, not looked up.** A freshly built, unsigned test binary refused on a Windows 11 Pro
/// 26200 machine with Smart App Control enforcing on 2026-08-13 produced exactly this, with the
/// message *"An Application Control policy has blocked this file"*; the events beside it were
/// `Microsoft-Windows-CodeIntegrity/Operational` 3033, 3077 and 3118. It is declared here with its
/// provenance rather than spelled as a `windows-sys` symbol, because that crate exports no name for
/// it.
///
/// **A lower bound.** This is the only code this project has observed for the condition, not a proof
/// that it is the only one. [`refused_by_app_control`] therefore fails towards silence: a diagnosis
/// that does not appear, never one that appears wrongly.
pub const APPLICATION_CONTROL_BLOCKED: i32 = 4551;

/// Why an image load was refused, in one sentence, for whoever reads the failure.
///
/// One definition, attached in two places — the post-install smoke test and the daemon's rendering
/// of a spawn that failed — so the two cannot drift into two explanations of one condition.
pub const APP_CONTROL_REFUSAL: &str = "an application control policy on this machine refused to \
                                       load it: MixEngine and the programs it starts are not \
                                       code-signed, and Windows' Code Integrity has no per-file \
                                       override to offer";

/// Was this failure an application control policy refusing to load the image?
///
/// **`cfg!` and not `#[cfg]`**, so the function compiles on all three systems and its tests run on
/// all three — including the one asserting it is `false` where the condition cannot arise.
#[must_use]
pub fn refused_by_app_control(error: &std::io::Error) -> bool {
    cfg!(windows) && error.raw_os_error() == Some(APPLICATION_CONTROL_BLOCKED)
}

/// What Smart App Control is doing on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppControlState {
    /// Nothing is refusing an image here for want of a signature.
    ///
    /// **Also what a machine with no such feature reads as** — Windows 10, Server, anything before
    /// Windows 11 22H2 — because the question is "is something refusing images here", and no policy
    /// is a clear no (T94 design, D5).
    Off,

    /// Smart App Control is watching this machine and has not decided yet.
    Evaluation,

    /// Smart App Control is enforcing: an unsigned image with no reputation is refused at load,
    /// with no warning and no override.
    Enforced,

    /// The policy value is one this build has no name for.
    Unknown {
        /// What was read, so a report carries it rather than a guess.
        value: u32,
    },
}

impl AppControlState {
    /// What `VerifiedAndReputablePolicyState` means, including its absence.
    #[must_use]
    pub fn from_policy_value(value: Option<u32>) -> Self {
        match value {
            None | Some(0) => Self::Off,
            Some(1) => Self::Enforced,
            Some(2) => Self::Evaluation,
            Some(value) => Self::Unknown { value },
        }
    }
}

/// What this machine's Smart App Control policy is doing — roadmap task **T94**.
///
/// **Reads only, and needs no privilege.** The value lives under
/// `HKLM\SYSTEM\CurrentControlSet\Control`, which any account may query, and nothing here writes —
/// which is what keeps `daemon.doctor`'s "nothing here writes" true with this check in it.
pub trait AppControl: std::fmt::Debug + Send + Sync {
    /// What this machine answers.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) on a system with no such
    /// mechanism, which `mix doctor` renders as a check that ran and says why it had nothing to
    /// examine; [`Error::Os`](crate::Error::Os) when the policy is there and could not be read.
    fn state(&self) -> Result<AppControlState>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two ends of this reader's own range, both taken on one machine: `1` while Smart App
    /// Control was enforcing (2026-08-13) and `0` after it had been turned off (2026-09-04).
    #[test]
    fn the_two_readings_this_project_has_actually_taken_are_the_two_they_were() {
        assert_eq!(
            AppControlState::from_policy_value(Some(1)),
            AppControlState::Enforced
        );
        assert_eq!(
            AppControlState::from_policy_value(Some(0)),
            AppControlState::Off
        );
    }

    /// A machine too old for Smart App Control has no key and no value, and that is a clear "no"
    /// rather than a question nobody asked — the design's D5.
    #[test]
    fn a_machine_with_no_such_policy_is_off_rather_than_unknown() {
        assert_eq!(
            AppControlState::from_policy_value(None),
            AppControlState::Off
        );
    }

    #[test]
    fn evaluation_is_its_own_answer() {
        assert_eq!(
            AppControlState::from_policy_value(Some(2)),
            AppControlState::Evaluation
        );
    }

    /// A build with no name for a state must not guess which named one it resembles, and it must
    /// carry the number so a bug report has it.
    #[test]
    fn a_state_this_build_has_no_name_for_keeps_its_number() {
        assert_eq!(
            AppControlState::from_policy_value(Some(7)),
            AppControlState::Unknown { value: 7 }
        );
    }

    /// The measured code, and only on the system it was measured on — the design's D4.
    #[test]
    fn the_measured_refusal_is_recognised_on_windows_and_nowhere_else() {
        let refusal = std::io::Error::from_raw_os_error(APPLICATION_CONTROL_BLOCKED);

        assert_eq!(refused_by_app_control(&refusal), cfg!(windows));
    }

    /// Silence rather than a wrong diagnosis: everything else answers no.
    #[test]
    fn an_ordinary_failure_is_not_read_as_a_policy_refusal() {
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        let no_os_code = std::io::Error::other("something this crate made up");

        assert!(!refused_by_app_control(&missing));
        assert!(!refused_by_app_control(&no_os_code));
    }
}
