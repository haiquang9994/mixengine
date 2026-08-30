//! Deciding that a service is holding more than it was allowed — roadmap task **T71a**.
//!
//! The reading is T71's: one `MetricsMinute` per subject per minute, already assembled by the
//! sampler. What is here is everything that reading cannot say for itself — how many minutes this
//! home is willing to see, whether this service's recipe permits a restart, and whether this episode
//! has already had one.
//!
//! # The arithmetic is in finished minutes, never in elapsed time
//!
//! A count is spent in *minutes the sampler completed*, exactly as an idle policy is spent in sweeps
//! that saw a service idle, and for the same reason `services::idle` writes out at length: tokio
//! measures from `Instant`, which counts no time while a laptop is suspended, so the first minute
//! after a lid opens can arrive eight hours late. A missing minute is therefore not a minute over the
//! line — it is *nobody measured*, and it takes the count back to zero.
//!
//! # One restart per episode
//!
//! A service that comes back and is immediately over its ceiling again is not restarted a second
//! time. It has to be seen **under** the line once before it may be restarted again, which makes a
//! pool that leaks up to its ceiling every twenty minutes a pool rescued every twenty minutes, and a
//! `memory_mb` set below what the service needs at boot exactly one restart followed by a service
//! left alone in `Degraded`. The second case is a misconfiguration, and a machine restarting a
//! database for ever is worse than a number nobody enforced.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mixengine_core::services::ServiceGraph;
use mixengine_proto::{Enforcement, MetricsMinute, MetricsSubject, ServiceId, StateReason};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::runner::Over;

/// How many bytes a megabyte is, for the one comparison this module makes.
const MB: u64 = 1024 * 1024;

/// What a finished minute concluded about one service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Under its ceiling, or not measured at all.
    ///
    /// **The two are one variant deliberately.** They are the same fact to everything downstream:
    /// there is nothing to warn about and nothing to act on. They differ in what they mean about the
    /// *count*, and that difference is spent inside [`Tally::observe`] rather than published.
    Under,

    /// Over the line, and either not for long enough yet or not a service anybody may restart.
    Warn {
        /// Consecutive finished minutes seen over the line, never more than `needed`.
        seen: u32,

        /// How many are needed before a restart.
        needed: u32,
    },

    /// Over the line for long enough, and its recipe permits it: restart.
    Restart,
}

/// What one service has been seen doing.
#[derive(Debug, Default)]
struct Seen {
    /// Consecutive finished minutes over the line, saturated at what was needed.
    ///
    /// **Saturated rather than climbing**, because a number that keeps rising past the threshold is
    /// a number nobody reads: `seen: 47 of 3` says nothing `seen: 3 of 3` does not.
    over: u32,

    /// Whether this episode has already had its restart.
    ///
    /// Cleared only by a minute under the line — see the module's second section.
    restarted: bool,
}

/// The consecutive-minute counts, per service.
///
/// **Held here and not in a column**, on `services::idle`'s reasoning: a daemon that restarts
/// forgets, which is correct, because a service it has just adopted has been observed zero times.
#[derive(Debug, Default)]
pub(crate) struct Tally {
    seen: BTreeMap<ServiceId, Seen>,
}

impl Tally {
    /// Fold one finished minute in, and say what follows.
    ///
    /// `rss_avg` of [`None`] is *this subject had no finished minute* — the service was stopped, the
    /// daemon was replaced, the machine was asleep. It is not the same fact as a reading under the
    /// line, and it resets the count for the reason the module header gives.
    ///
    /// `needed` is floored at one: a home configured with zero would otherwise restart a service on
    /// its first finished minute, which at the idle sample rate is a single instantaneous reading.
    pub(crate) fn observe(
        &mut self,
        id: &ServiceId,
        limit_mb: u32,
        needed: u32,
        may_restart: bool,
        rss_avg: Option<u64>,
    ) -> Verdict {
        let needed = needed.max(1);

        // `u64::from` rather than a cast: a `memory_mb` of four million would wrap a `u32` of bytes
        // and turn the largest ceiling anybody could set into the smallest.
        let over_line = rss_avg.is_some_and(|rss| rss > u64::from(limit_mb) * MB);

        if !over_line {
            self.seen.remove(id);
            return Verdict::Under;
        }

        let entry = self.seen.entry(id.clone()).or_default();
        entry.over = entry.over.saturating_add(1).min(needed);

        if entry.over >= needed && may_restart && !entry.restarted {
            entry.restarted = true;
            return Verdict::Restart;
        }

        Verdict::Warn {
            seen: entry.over,
            needed,
        }
    }

    /// Drop what was counted for a service this daemon is no longer supervising.
    ///
    /// A service that stopped and was started again begins at zero rather than resuming a count
    /// taken against a process that no longer exists.
    pub(crate) fn forget(&mut self, id: &ServiceId) {
        self.seen.remove(id);
    }

    /// Every service this tally is currently counting, so a caller can forget the rest.
    pub(crate) fn watching(&self) -> impl Iterator<Item = &ServiceId> {
        self.seen.keys()
    }
}

/// Everything the watchdog needs to decide and to act.
///
/// **It has no clock.** A sweeper of T69's kind owns an interval; this owns a receiver, and its
/// period *is* the sampler's — which is what keeps exactly one thing on this machine reading the
/// process table.
#[derive(Debug)]
pub(crate) struct Watchdog {
    registry: Arc<super::Registry>,
    tally: Tally,

    /// How many finished minutes over the line a service is given, from this home's config.
    needed: u32,
}

impl Watchdog {
    /// A watchdog over this home's services.
    pub(crate) fn new(registry: Arc<super::Registry>, needed: u32) -> Self {
        Self {
            registry,
            tally: Tally::default(),
            needed,
        }
    }

    /// Fold one minute's rows in, and act on what they say.
    pub(crate) async fn minute(&mut self, rows: &[MetricsMinute]) {
        // **The machine's answer, never the operating system's name.** Where a kernel holds the
        // ceiling there is nothing to watch, and two things judging one number by two different
        // quantities would disagree in public.
        if matches!(
            self.registry.host().resource_control().support().memory,
            Enforcement::Hard { .. }
        ) {
            return;
        }

        let graph = match self.registry.graph().await {
            Ok(graph) => graph,

            // Debug and not warn, on `idle`'s reasoning: this arrives every minute, and a home whose
            // services cannot be read already says so at start and in `mix doctor`.
            Err(error) => {
                tracing::debug!(
                    ?error,
                    "this home's services could not be read, so nothing is watched"
                );
                return;
            }
        };

        let running = self.registry.supervised();
        self.forget_everything_but(&running);

        for id in &running {
            let Some(spec) = graph.spec(id) else { continue };
            let Some(limit_mb) = spec.limits().memory_mb else {
                continue;
            };

            // **`None` where this subject has no row this minute**, which is not a reading under the
            // line: see `Tally::observe`.
            let rss_avg = rows
                .iter()
                .find(
                    |row| matches!(&row.subject, MetricsSubject::Service(subject) if subject == id),
                )
                .map(|row| row.rss_avg);

            let may_restart = spec.restart_over_memory();
            let verdict = self
                .tally
                .observe(id, limit_mb, self.needed, may_restart, rss_avg);

            let over = Over {
                rss_bytes: rss_avg.unwrap_or_default(),
                limit_mb,
            };

            match verdict {
                Verdict::Under => {
                    self.registry.over_memory(id, None);
                }

                // Sent every minute, and the runner writes a transition only when the *state*
                // changes — so a service that stays over its ceiling is one row in
                // `service_transitions` rather than one per minute.
                Verdict::Warn { seen, needed } => {
                    self.registry.over_memory(id, Some(over));

                    tracing::debug!(
                        service = id.as_str(),
                        seen,
                        needed,
                        limit_mb,
                        "this service is over its memory ceiling"
                    );
                }

                Verdict::Restart => {
                    self.registry.over_memory(id, Some(over));
                    self.restart(&graph, id, over).await;
                }
            }
        }
    }

    /// Restart one service that stayed over its ceiling.
    ///
    /// The walk a person's `mix service restart` takes, and not a second one — see
    /// [`super::restart`]: two implementations of *stop this and start it again* would be two sets
    /// of edge cases about dependents, drifting apart the day a recipe declares one.
    async fn restart(&self, graph: &ServiceGraph, id: &ServiceId, over: Over) {
        // **Set before the walk, never after** — `idle::Sweeper::stop` says why: the runner reads
        // this at the moment it enters `Stopping`, so a value written afterwards explains the wrong
        // stop, and most likely mislabels a person's own `mix service stop` as this one.
        self.registry.stopping_because(
            id,
            Some(StateReason::OverMemory {
                rss_bytes: over.rss_bytes,
                limit_mb: over.limit_mb,
            }),
        );

        if super::restart(&self.registry, graph, id).await.is_none() {
            // Taken back on every path that did not stop it, on the sweeper's rule.
            self.registry.stopping_because(id, None);

            tracing::warn!(
                service = id.as_str(),
                "a service over its memory ceiling did not restart"
            );

            return;
        }

        tracing::info!(
            service = id.as_str(),
            rss_bytes = over.rss_bytes,
            limit_mb = over.limit_mb,
            minutes = self.needed,
            "this service stayed over its memory ceiling, so it was restarted"
        );
    }

    /// Drop the count of every service this daemon is no longer supervising.
    fn forget_everything_but(&mut self, running: &BTreeSet<ServiceId>) {
        let counted: Vec<ServiceId> = self.tally.watching().cloned().collect();

        for id in counted {
            if !running.contains(&id) {
                self.tally.forget(&id);
            }
        }
    }
}

/// Watch every minute the sampler finishes, until `shutdown`.
///
/// **No interval of its own.** The three loops beside this one in `main.rs` each own a clock; this
/// one owns a receiver, so its rate is the sampler's and there is still exactly one thing on this
/// machine reading the process table.
pub(crate) fn start(
    mut watchdog: Watchdog,
    mut minutes: broadcast::Receiver<Vec<MetricsMinute>>,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let received = tokio::select! {
                () = shutdown.cancelled() => return,
                received = minutes.recv() => received,
            };

            match received {
                Ok(rows) => watchdog.minute(&rows).await,

                // **Minutes missed are minutes nobody measured**, which resets a count — the safe
                // direction, and why this is `debug` rather than a warning. Treating them as
                // still-over would restart a service on evidence discarded for being old.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::debug!(missed, "the memory watchdog fell behind the sampler");
                }

                // The sampler is gone, which happens only as this daemon ends.
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT_MB: u32 = 512;
    const NEEDED: u32 = 3;

    /// Comfortably under and comfortably over a 512 MB ceiling.
    const UNDER: u64 = 400 * MB;
    const OVER: u64 = 600 * MB;

    fn id(text: &str) -> ServiceId {
        ServiceId::parse(text).expect("a valid id")
    }

    /// The count is spent in consecutive minutes, and the last one restarts it.
    #[test]
    fn three_consecutive_minutes_over_the_line_restart_it() {
        let mut tally = Tally::default();
        let pool = id("php-fpm@8.3");

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Warn { seen: 1, needed: 3 }
        );
        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Warn { seen: 2, needed: 3 }
        );
        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Restart
        );
    }

    /// Exactly at the ceiling is not over it.
    ///
    /// A service told it may have 512 MB and holding 512 MB is a service doing as it was told.
    #[test]
    fn a_service_at_its_ceiling_is_not_over_it() {
        let mut tally = Tally::default();

        assert_eq!(
            tally.observe(&id("php-fpm@8.3"), LIMIT_MB, NEEDED, true, Some(512 * MB)),
            Verdict::Under
        );
    }

    /// One minute under the line is the episode over.
    #[test]
    fn a_minute_under_the_line_resets_the_count() {
        let mut tally = Tally::default();
        let pool = id("php-fpm@8.3");

        tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));
        tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(UNDER)),
            Verdict::Under
        );

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Warn { seen: 1, needed: 3 },
            "the count starts again rather than resuming where it was"
        );
    }

    /// A minute nobody measured is not a minute over the line.
    ///
    /// The safety property, and it is T69's: a machine that slept eight hours wakes with a count of
    /// zero, not with eight hours of evidence.
    #[test]
    fn a_missing_minute_resets_the_count() {
        let mut tally = Tally::default();
        let pool = id("php-fpm@8.3");

        tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));
        tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, None),
            Verdict::Under
        );

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Warn { seen: 1, needed: 3 }
        );
    }

    /// One restart per episode, and an episode ends by being seen under the line.
    #[test]
    fn a_service_that_comes_back_still_over_is_restarted_once_and_then_left_alone() {
        let mut tally = Tally::default();
        let pool = id("php-fpm@8.3");

        for _ in 1..NEEDED {
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));
        }
        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Restart
        );

        for _ in 0..10 {
            assert_eq!(
                tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
                Verdict::Warn { seen: 3, needed: 3 },
                "a ceiling below what the service needs at boot is a misconfiguration, not a leak"
            );
        }

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(UNDER)),
            Verdict::Under
        );

        for _ in 1..NEEDED {
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));
        }
        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Restart,
            "an episode that ended and began again earns its own restart"
        );
    }

    /// A service its recipe will not let anybody restart is warned about, for as long as it lasts.
    #[test]
    fn a_service_that_may_not_be_restarted_never_leaves_warn() {
        let mut tally = Tally::default();
        let database = id("mariadb@main");

        for _ in 0..20 {
            assert!(matches!(
                tally.observe(&database, LIMIT_MB, NEEDED, false, Some(OVER)),
                Verdict::Warn { .. }
            ));
        }
    }

    /// A home that asked for zero minutes still gets one, never none.
    #[test]
    fn a_count_of_zero_minutes_is_still_one_minute() {
        let mut tally = Tally::default();

        assert_eq!(
            tally.observe(&id("php-fpm@8.3"), LIMIT_MB, 0, true, Some(OVER)),
            Verdict::Restart
        );
    }

    /// A service that stopped is forgotten, so its next life begins at zero.
    #[test]
    fn forgetting_a_service_clears_what_was_counted() {
        let mut tally = Tally::default();
        let pool = id("php-fpm@8.3");

        tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));
        tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER));
        tally.forget(&pool);

        assert_eq!(
            tally.observe(&pool, LIMIT_MB, NEEDED, true, Some(OVER)),
            Verdict::Warn { seen: 1, needed: 3 }
        );
    }

    /// What is being counted is what a caller may need to forget.
    #[test]
    fn a_tally_says_which_services_it_is_counting() {
        let mut tally = Tally::default();

        tally.observe(&id("php-fpm@8.3"), LIMIT_MB, NEEDED, true, Some(OVER));
        tally.observe(&id("mariadb@main"), LIMIT_MB, NEEDED, false, Some(UNDER));

        let watched: Vec<&str> = tally.watching().map(ServiceId::as_str).collect();
        assert_eq!(
            watched,
            ["php-fpm@8.3"],
            "a service under its ceiling is not being counted"
        );
    }
}
