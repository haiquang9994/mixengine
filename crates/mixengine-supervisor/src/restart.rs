//! What to do when a supervised process ends.
//!
//! Three answers, and they are the three states a service can be left in — which is not a
//! coincidence but the reason the type has three variants. A service that ended the way it was meant
//! to is [`Decision::Rest`] and `Stopped`; one that is going to be started again is
//! [`Decision::Restart`] and `Restarting`; one nothing is going to be done about is
//! [`Decision::GiveUp`] and `Failed`.
//!
//! # Why a crash loop is counted in a window
//!
//! Retrying forever against a port that is never going to be free is the failure mode this exists to
//! prevent, and the naive fix — a total attempt limit — is wrong in the other direction: a service
//! that crashes once a day and is restarted would exhaust a budget of five some time in the second
//! week and stay `Failed` for no reason anybody could reconstruct. So failures are counted *inside a
//! window*, and one that falls out of it is forgotten.
//!
//! Becoming healthy again does **not** clear that history, which is deliberate. A service that
//! starts, works for four seconds and dies, five times in a minute, is exactly a crash loop — and
//! the version that cleared the count on every success would restart it forever while reporting
//! that everything was fine. What recovery does reset is the *backoff*, because the next wait should
//! start at half a second again rather than at thirty.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mixengine_platform::process::Exit;
use mixengine_proto::{Backoff, Millis, RestartPolicy, StateReason};

use crate::logs::Capture;

/// How many lines of a service's output are attached to a crash-loop failure.
///
/// The number `.claude/architecture/process-supervision.md` names. Enough to hold a stack trace and
/// the line above it that says what was actually wrong; small enough that an event carrying one is
/// still an event.
pub const TAIL_LINES: usize = 200;

/// What the supervisor does now that a process has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Start it again, after waiting this long. The service is `Restarting`.
    Restart {
        /// How long to wait first — the backoff, scattered by the policy's jitter.
        after: Duration,

        /// Which attempt the next start will be, counted from 1 since the service was last healthy.
        ///
        /// What makes "restarting (3 of 5)" possible, which is the sentence that tells a user
        /// whether to keep waiting or go and look at something.
        attempt: u32,
    },

    /// Leave it stopped. It ended the way it was supposed to. The service is `Stopped`.
    Rest {
        /// Why, for the transition that records it.
        reason: StateReason,
    },

    /// Stop trying. The service is `Failed` until somebody starts it explicitly.
    GiveUp {
        /// Why — and for a crash loop, with the last lines the service printed.
        reason: StateReason,
    },
}

/// The restart history of one service, judged against its policy.
#[derive(Debug)]
pub struct Restarts {
    policy: RestartPolicy,

    /// When each recent failure happened, oldest first. Trimmed to the policy's window on every
    /// crash, so it holds at most a handful of moments however long the service runs.
    failures: VecDeque<Instant>,

    /// How many times this service has been restarted since it was last healthy.
    attempt: u32,
}

impl Restarts {
    /// Start counting for a service running under `policy`.
    #[must_use]
    pub fn under(policy: RestartPolicy) -> Self {
        Self {
            policy,
            failures: VecDeque::new(),
            attempt: 0,
        }
    }

    /// The service is up and well again.
    ///
    /// Resets the backoff — the next crash waits half a second, not thirty — and deliberately keeps
    /// the failure history, because a service that recovers between crashes is still crashing. See
    /// the module documentation.
    pub fn recovered(&mut self) {
        self.attempt = 0;
    }

    /// The process ended at `at`. Decide what happens next.
    ///
    /// `logs` is read only when the answer is [`Decision::GiveUp`] for a crash loop, which is the
    /// one reason that cannot explain itself.
    ///
    /// `at` is passed in rather than read from the clock for the same reason
    /// `mixengine_core::services::transition` takes its timestamp: the caller already has a reading,
    /// and a test needs to be able to say when.
    pub fn ended(&mut self, exit: &Exit, at: Instant, logs: &Capture) -> Decision {
        let ended = StateReason::Exited { code: exit.code() };

        let (retries, window, backoff) = match self.policy {
            // Nothing to decide. A service told never to restart is left exactly as it ended, and
            // whether that reads as "stopped" or "failed" is the exit's business, not the policy's.
            RestartPolicy::Never => {
                return if exit.is_success() {
                    Decision::Rest { reason: ended }
                } else {
                    Decision::GiveUp { reason: ended }
                };
            }

            RestartPolicy::OnFailure {
                max_retries,
                window,
                backoff,
            } => {
                // The distinction this policy exists for: a service that exited zero did what it was
                // asked to. Restarting it would be a supervisor arguing with the program.
                if exit.is_success() {
                    return Decision::Rest { reason: ended };
                }

                (Some(max_retries), window, backoff)
            }

            // No ceiling on purpose: this is for a service whose absence is itself the failure, and
            // the backoff is what keeps "forever" from meaning "in a tight loop".
            RestartPolicy::Always { backoff } => (None, Millis(0), backoff),

            // `RestartPolicy` is `#[non_exhaustive]`. A policy this build does not know is not a
            // licence to invent one: the service is left where it is, which is the only answer that
            // cannot do damage.
            ref other => {
                tracing::warn!(policy = ?other, "unknown restart policy; leaving the service alone");

                return Decision::GiveUp { reason: ended };
            }
        };

        if let Some(max_retries) = retries {
            self.remember(at, window);

            let attempts = u32::try_from(self.failures.len()).unwrap_or(u32::MAX);

            if attempts > max_retries {
                return Decision::GiveUp {
                    reason: StateReason::CrashLoop {
                        attempts,
                        window,
                        tail: logs
                            .recent(TAIL_LINES)
                            .into_iter()
                            .map(|line| line.text)
                            .collect(),
                    },
                };
            }
        }

        self.attempt += 1;

        Decision::Restart {
            after: wait_for(backoff, self.attempt),
            attempt: self.attempt,
        }
    }

    /// Record a failure and forget the ones that have aged out of the window.
    fn remember(&mut self, at: Instant, window: Millis) {
        let window = window.as_duration();

        while self
            .failures
            .front()
            .is_some_and(|failure| at.saturating_duration_since(*failure) > window)
        {
            self.failures.pop_front();
        }

        self.failures.push_back(at);
    }
}

/// How long to wait before attempt number `attempt`, counted from 1.
///
/// Exponential from `initial`, multiplied by an integer percentage each time, capped at `max`, and
/// then scattered by the jitter. Saturating throughout: a manifest asking for a multiplier of
/// 4 000 000 % produces the ceiling, not a panic and not a wait of two milliseconds from an
/// overflow that wrapped.
fn wait_for(backoff: Backoff, attempt: u32) -> Duration {
    let max = backoff.max.0;
    let mut wait = backoff.initial.0.min(max);

    for _ in 1..attempt {
        wait = wait
            .saturating_mul(u64::from(backoff.multiplier_percent))
            .saturating_div(100)
            .min(max);
    }

    Duration::from_millis(scatter(wait, backoff.jitter_percent))
}

/// Spread `wait` by ±`percent`, so services restarting together do not synchronise.
///
/// The one place in the tree that needs randomness, and it needs very little of it: what a
/// synchronised herd of retries requires to break up is any spread at all, not an unpredictable one.
/// So this is eight lines of xorshift over a process-wide seed rather than a dependency — nothing
/// here is a nonce, a key or an identifier, and the day something is, it will want a real generator
/// and should not find this one lying about.
fn scatter(wait: u64, percent: u8) -> u64 {
    // Multiplied before it is divided, which is not pedantry: the other order truncates to zero for
    // every wait under 100 ms, so a spec with a short `initial` — legal, since only zero is refused
    // — would silently get no jitter at all, and a herd of services restarting together is exactly
    // what a short backoff produces most of.
    let spread = wait.saturating_mul(u64::from(percent)) / 100;

    if spread == 0 {
        return wait;
    }

    // `1 + 2 * spread` values, centred on `wait`: the range is closed at both ends, so a jitter of
    // 20 % really can produce 0.8× and 1.2× rather than stopping just short of them.
    wait.saturating_sub(spread) + random() % (2 * spread + 1)
}

/// A pseudo-random `u64`, from a seed nobody chose.
///
/// xorshift64*, which is two shifts and a multiply and is uniform enough for a jitter. The seed is
/// the clock the first time it is asked for; every process gets a different one, which is all that
/// matters here — two daemons on one machine are not what a herd of retries is made of.
fn random() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static STATE: AtomicU64 = AtomicU64::new(0);

    let mut seed = STATE.load(Ordering::Relaxed);

    if seed == 0 {
        seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0x2545_f491_4f6c_dd1d, |since| since.as_nanos() as u64)
            // A clock that lands exactly on zero nanoseconds would otherwise reseed forever.
            | 1;
    }

    seed ^= seed >> 12;
    seed ^= seed << 25;
    seed ^= seed >> 27;

    STATE.store(seed, Ordering::Relaxed);

    seed.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The policy itself is tested in `tests/restart.rs`, against exit statuses produced by real
    // processes: `Exit` describes something that happened and has no constructor, which is worth
    // keeping — a shipped API with a way to invent an exit status is a way to report one. What is
    // left here is the arithmetic, which needs no process at all.

    fn backoff() -> Backoff {
        Backoff {
            initial: Millis(500),
            max: Millis::from_secs(30),
            multiplier_percent: 200,
            jitter_percent: 0,
        }
    }

    #[test]
    fn a_backoff_doubles_from_its_initial_wait_and_stops_at_the_ceiling() {
        let waits: Vec<u64> = (1..=8)
            .map(|attempt| wait_for(backoff(), attempt).as_millis() as u64)
            .collect();

        assert_eq!(
            waits,
            [500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]
        );
    }

    #[test]
    fn a_multiplier_that_would_overflow_lands_on_the_ceiling() {
        let absurd = Backoff {
            multiplier_percent: u32::MAX,
            ..backoff()
        };

        assert_eq!(wait_for(absurd, 5), Duration::from_secs(30));
    }

    /// Jitter has to stay inside its own bounds, and has to actually vary.
    #[test]
    fn jitter_scatters_within_the_percentage_it_was_given() {
        let jittered = Backoff {
            jitter_percent: 20,
            ..backoff()
        };

        let waits: Vec<u64> = (0..200)
            .map(|_| wait_for(jittered, 3).as_millis() as u64)
            .collect();

        assert!(
            waits.iter().all(|wait| (1_600..=2_400).contains(wait)),
            "a wait fell outside ±20 % of 2 s: {waits:?}"
        );
        assert!(
            waits.iter().any(|wait| *wait != waits[0]),
            "every wait was identical, so nothing is being scattered and a herd of services would \
             still restart in step"
        );
    }

    /// A short backoff is still scattered — the case integer division silently used to flatten.
    ///
    /// Fifty milliseconds is a legal `initial` (only zero is refused), and a short wait is where a
    /// herd matters most: the shorter the backoff, the more likely two services that crashed
    /// together come back in the same instant.
    #[test]
    fn a_backoff_too_short_to_divide_by_a_hundred_is_scattered_anyway() {
        let brief = Backoff {
            initial: Millis(50),
            max: Millis(50),
            jitter_percent: 20,
            ..backoff()
        };

        let waits: Vec<u64> = (0..200)
            .map(|_| wait_for(brief, 1).as_millis() as u64)
            .collect();

        assert!(
            waits.iter().all(|wait| (40..=60).contains(wait)),
            "a wait fell outside ±20 % of 50 ms: {waits:?}"
        );
        assert!(
            waits.iter().any(|wait| *wait != waits[0]),
            "a 50 ms backoff was not scattered at all, so two services that crashed together come \
             back in the same instant: {waits:?}"
        );
    }

    #[test]
    fn no_jitter_means_no_scatter() {
        assert_eq!(wait_for(backoff(), 1), Duration::from_millis(500));
    }
}
