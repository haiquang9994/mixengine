//! Whether a release is offered to this machine, and if not, why — roadmap task **T88**.
//!
//! **The decision is the daemon's and the sentence is the daemon's.** `.claude/features/updates.md`
//! and `CLAUDE.md` between them say a client renders what it is given and derives nothing, so the
//! four reasons a perfectly good release is not offered — it is not newer, there is no build for
//! this machine, somebody skipped it, somebody asked to be reminded later — arrive as whole
//! sentences rather than as codes every client would have to spell back out in English.
//!
//! Nothing here reads a clock or a database: both arrive as arguments, which is what makes every
//! branch below testable without either.

use mixengine_proto::{PackageVersion, Timestamp};

use super::records::REMIND_CLAMP_SECONDS;

/// What a daemon decided about one release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Whether the client should show it.
    pub offered: bool,

    /// Why not, phrased for a person. [`None`] exactly when [`Decision::offered`] is `true`.
    pub because: Option<String>,
}

impl Decision {
    /// Offered, with nothing to explain.
    fn yes() -> Self {
        Self {
            offered: true,
            because: None,
        }
    }

    /// Not offered, and this is the sentence.
    fn no(because: impl Into<String>) -> Self {
        Self {
            offered: false,
            because: Some(because.into()),
        }
    }
}

/// Decide whether `offered_version` is shown to somebody running `current`.
///
/// `has_build_for_this_machine` is the answer [`super::Feed::artifact`] gave for this machine's
/// `(os, arch)` — passed in rather than looked up, because the pair is `main`'s to establish and a
/// decision that consulted `std::env::consts` would be a decision no test could vary.
///
/// The order of the checks is the order a person would ask them in, and it decides which sentence
/// they get when more than one applies: *is there anything newer* comes before *did you say no to
/// it*, because a machine that is already up to date should not be told it once skipped something.
#[must_use]
pub fn decide(
    current: &str,
    offered_version: &str,
    has_build_for_this_machine: bool,
    skipped: Option<&str>,
    remind_after: Option<Timestamp>,
    now: Timestamp,
) -> Decision {
    let (Ok(running), Ok(offered)) = (
        PackageVersion::parse(current),
        PackageVersion::parse(offered_version),
    ) else {
        // Unreachable from a signed feed and a `CARGO_PKG_VERSION`, and reported rather than
        // assumed away: a build whose own version will not parse must not offer to replace itself
        // with something it cannot compare against.
        return Decision::no(format!(
            "this build is {current} and the published release is {offered_version}, and one of \
             those is not a version this build can compare"
        ));
    };

    if offered.cmp_precedence(&running).is_le() {
        return Decision::no(format!(
            "{offered_version} is not newer than the {current} this machine is running"
        ));
    }

    if !has_build_for_this_machine {
        return Decision::no(format!(
            "{offered_version} has no build for this machine's operating system and architecture"
        ));
    }

    if skipped == Some(offered_version) {
        return Decision::no(format!("you skipped {offered_version}"));
    }

    if let Some(due) = remind_after {
        // **Ignored rather than clamped when it is absurdly far ahead**, which is the difference
        // between a rule that works and one that reads as though it does. A machine whose clock was
        // a year fast when somebody answered *later* holds a moment a year away once the clock is
        // corrected; clamping that to `now + seven days` would move the deadline forward on every
        // read and it would never come due at all. So a moment beyond the clamp is not believed to
        // be a reminder, and the release is offered — early, which is the harmless direction.
        let ahead = due.0.saturating_sub(now.0);

        if ahead > 0 && ahead <= REMIND_CLAMP_SECONDS * 1_000 {
            return Decision::no(format!(
                "you asked to be reminded about updates later, and that is not yet ({})",
                due.to_rfc3339()
            ));
        }
    }

    Decision::yes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> Timestamp {
        Timestamp::parse_rfc3339(text).expect("a timestamp")
    }

    #[test]
    fn a_newer_version_with_a_build_for_this_machine_is_offered() {
        let decision = decide(
            "0.1.0",
            "0.2.0",
            true,
            None,
            None,
            at("2026-09-05T00:00:00Z"),
        );

        assert!(decision.offered);
        assert_eq!(decision.because, None);
    }

    #[test]
    fn the_version_this_build_already_is_is_not_offered() {
        let decision = decide(
            "0.2.0",
            "0.2.0",
            true,
            None,
            None,
            at("2026-09-05T00:00:00Z"),
        );

        assert!(!decision.offered);
        assert!(decision.because.expect("a reason").contains("0.2.0"));
    }

    #[test]
    fn a_release_older_than_this_build_is_not_offered() {
        let decision = decide(
            "0.3.0",
            "0.2.0",
            true,
            None,
            None,
            at("2026-09-05T00:00:00Z"),
        );

        assert!(!decision.offered);
    }

    /// T85a builds five legs, so a sixth machine asking is a real case: it is told which pair has
    /// no build rather than being told there is no update.
    #[test]
    fn a_release_with_no_build_for_this_machine_says_so() {
        let decision = decide(
            "0.1.0",
            "0.2.0",
            false,
            None,
            None,
            at("2026-09-05T00:00:00Z"),
        );

        assert!(!decision.offered);
        assert!(
            decision
                .because
                .expect("a reason")
                .contains("no build for this machine")
        );
    }

    #[test]
    fn a_version_that_was_skipped_is_not_offered_again() {
        let decision = decide(
            "0.1.0",
            "0.2.0",
            true,
            Some("0.2.0"),
            None,
            at("2026-09-05T00:00:00Z"),
        );

        assert!(!decision.offered);
        assert!(decision.because.expect("a reason").contains("skipped"));
    }

    /// Skipping one version does not skip the next one, which is the whole difference between
    /// *skip this version* and *stop telling me about updates*.
    #[test]
    fn a_later_version_than_the_skipped_one_is_offered() {
        let decision = decide(
            "0.1.0",
            "0.3.0",
            true,
            Some("0.2.0"),
            None,
            at("2026-09-05T00:00:00Z"),
        );

        assert!(decision.offered);
    }

    #[test]
    fn a_reminder_that_has_not_come_due_suppresses_the_offer() {
        let decision = decide(
            "0.1.0",
            "0.2.0",
            true,
            None,
            Some(at("2026-09-06T00:00:00Z")),
            at("2026-09-05T00:00:00Z"),
        );

        assert!(!decision.offered);
    }

    #[test]
    fn a_reminder_that_has_come_due_does_not() {
        let decision = decide(
            "0.1.0",
            "0.2.0",
            true,
            None,
            Some(at("2026-09-04T00:00:00Z")),
            at("2026-09-05T00:00:00Z"),
        );

        assert!(decision.offered);
    }

    /// A clock corrected forward by a year must not suppress every future offer. A moment beyond
    /// the clamp is not a reminder anybody asked for, so it is ignored and the release is offered.
    #[test]
    fn a_reminder_a_year_ahead_is_not_believed_to_be_a_reminder() {
        let decision = decide(
            "0.1.0",
            "0.2.0",
            true,
            None,
            Some(at("2027-09-05T00:00:00Z")),
            at("2026-09-20T00:00:00Z"),
        );

        assert!(decision.offered, "{:?}", decision.because);
    }

    /// And the reminder somebody actually asked for is inside the clamp, so it still suppresses.
    /// This is the test that would fail if the clamp were ever narrowed below
    /// [`super::super::records::LATER_SECONDS`].
    #[test]
    fn the_reminder_this_product_writes_is_inside_the_clamp() {
        let now = at("2026-09-05T00:00:00Z");
        let due = Timestamp(now.0 + super::super::records::LATER_SECONDS * 1_000);

        let decision = decide("0.1.0", "0.2.0", true, None, Some(due), now);

        assert!(!decision.offered);
    }

    /// The one branch a signed feed cannot reach, and the reason it is a refusal rather than an
    /// `expect`: nothing in this crate panics.
    #[test]
    fn a_version_that_will_not_parse_is_not_offered() {
        let decision = decide("0.1.0", "", true, None, None, at("2026-09-05T00:00:00Z"));

        assert!(!decision.offered);
    }
}
