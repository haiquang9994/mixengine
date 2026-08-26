//! What the supervisor decides after a process has ended, against exit statuses it really produced.
//!
//! `Exit` has no constructor: it describes something that happened, and an API with a way to invent
//! one is an API with a way to report a crash that never occurred. So every status here comes from a
//! `fakeservice` that was asked to exit with it — one process per status, cloned for the tests that
//! need the same one several times over.
//!
//! The clock is not: `Restarts::ended` takes the moment as an argument, so the window tests move
//! time by arithmetic instead of by sleeping through five minutes of it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use mixengine_platform::process::{Exit, Limits, spawn_supervised};
use mixengine_proto::{Backoff, LogPolicy, Millis, RestartPolicy, ServiceId, StateReason};
use mixengine_supervisor::logs::Capture;
use mixengine_supervisor::restart::{Decision, Restarts};
use mixengine_testkit::FakeService;

/// A real exit status, and whatever the service printed on its way to it.
fn ended_with(code: i32, fixture: FakeService) -> (Exit, Capture) {
    let service = ServiceId::parse("fakeservice").expect("a valid id");
    let mut supervised = spawn_supervised(
        &FakeService::program(),
        fixture.exit_after(50).exit_code(code).args(),
        &std::env::temp_dir(),
        &BTreeMap::new(),
        &Limits::default(),
    )
    .expect("a fakeservice can be supervised");

    let mut logs = Capture::start(&mut supervised, &service, LogPolicy::default(), None);
    let exit = supervised.wait().expect("the fixture exits on its own");

    // This fixture leaves nothing holding its pipes, so end of file follows its exit and the wait is
    // a formality. The deadline is generous rather than tight because what it guards against is a
    // hang, not slowness — see `Capture::finish`.
    assert!(
        logs.finish(Duration::from_secs(20)),
        "the fixture left something holding its output open"
    );

    (exit, logs)
}

/// An exit status on its own, for the tests that are about the policy rather than the output.
fn exit_with(code: i32) -> Exit {
    ended_with(code, FakeService::new()).0
}

fn on_failure(max_retries: u32) -> RestartPolicy {
    RestartPolicy::OnFailure {
        max_retries,
        window: Millis::from_secs(300),
        backoff: Backoff {
            initial: Millis(500),
            max: Millis::from_secs(30),
            multiplier_percent: 200,
            jitter_percent: 0,
        },
    }
}

#[test]
fn a_service_that_exited_cleanly_is_left_stopped() {
    let mut restarts = Restarts::under(on_failure(5));
    let logs = Capture::detached();

    let decision = restarts.ended(&exit_with(0), Instant::now(), &logs);

    assert_eq!(
        decision,
        Decision::Rest {
            reason: StateReason::Exited { code: Some(0) }
        },
        "a service that did what it was asked was restarted anyway"
    );
}

#[test]
fn a_service_that_crashed_is_restarted_after_its_backoff() {
    let mut restarts = Restarts::under(on_failure(5));
    let logs = Capture::detached();
    let crash = exit_with(3);

    assert_eq!(
        restarts.ended(&crash, Instant::now(), &logs),
        Decision::Restart {
            after: Duration::from_millis(500),
            attempt: 1
        }
    );
    assert_eq!(
        restarts.ended(&crash, Instant::now(), &logs),
        Decision::Restart {
            after: Duration::from_secs(1),
            attempt: 2
        },
        "the second attempt waits twice as long as the first"
    );
}

/// The whole point of the cutoff, with the evidence attached.
#[test]
fn a_service_that_keeps_crashing_is_given_up_on_with_the_last_lines_it_printed() {
    let (crash, logs) = ended_with(1, FakeService::new().log_every(5));
    let mut restarts = Restarts::under(on_failure(3));
    let at = Instant::now();

    for attempt in 1..=3 {
        assert!(
            matches!(
                restarts.ended(&crash, at, &logs),
                Decision::Restart { attempt: n, .. } if n == attempt
            ),
            "attempt {attempt} was not a restart"
        );
    }

    let Decision::GiveUp {
        reason:
            StateReason::CrashLoop {
                attempts,
                window,
                tail,
            },
    } = restarts.ended(&crash, at, &logs)
    else {
        panic!("a fourth crash inside the window was not a crash loop");
    };

    assert_eq!(attempts, 4);
    assert_eq!(window, Millis::from_secs(300));
    assert!(
        tail.iter().any(|line| line.contains("fakeservice: line")),
        "the failure carries no evidence, which is the whole reason it carries anything: {tail:?}"
    );
}

/// A service that crashes once a day is not in a crash loop.
#[test]
fn a_failure_that_has_aged_out_of_the_window_is_forgotten() {
    let mut restarts = Restarts::under(on_failure(2));
    let logs = Capture::detached();
    let crash = exit_with(1);
    let start = Instant::now();

    restarts.ended(&crash, start, &logs);
    restarts.ended(&crash, start + Duration::from_secs(1), &logs);

    // Six minutes later, with a five-minute window: the two above are history.
    let decision = restarts.ended(&crash, start + Duration::from_secs(360), &logs);

    assert!(
        matches!(decision, Decision::Restart { .. }),
        "a failure outside the window counted towards the budget: {decision:?}"
    );
}

/// Recovery resets the wait and not the history — see the module's own documentation for why.
#[test]
fn recovering_resets_the_backoff_but_not_the_crash_count() {
    let mut restarts = Restarts::under(on_failure(2));
    let logs = Capture::detached();
    let crash = exit_with(1);
    let at = Instant::now();

    restarts.ended(&crash, at, &logs);
    restarts.ended(&crash, at, &logs);
    restarts.recovered();

    let decision = restarts.ended(&crash, at, &logs);

    assert!(
        matches!(decision, Decision::GiveUp { .. }),
        "a service that recovers between crashes escaped the cutoff forever: {decision:?}"
    );

    let mut restarts = Restarts::under(on_failure(5));
    restarts.ended(&crash, at, &logs);
    restarts.ended(&crash, at, &logs);
    restarts.recovered();

    assert_eq!(
        restarts.ended(&crash, at, &logs),
        Decision::Restart {
            after: Duration::from_millis(500),
            attempt: 1
        },
        "the wait after a recovery started where the previous run left off"
    );
}

#[test]
fn a_service_told_never_to_restart_never_does() {
    let logs = Capture::detached();

    assert_eq!(
        Restarts::under(RestartPolicy::Never).ended(&exit_with(0), Instant::now(), &logs),
        Decision::Rest {
            reason: StateReason::Exited { code: Some(0) }
        }
    );
    assert_eq!(
        Restarts::under(RestartPolicy::Never).ended(&exit_with(7), Instant::now(), &logs),
        Decision::GiveUp {
            reason: StateReason::Exited { code: Some(7) }
        },
        "a one-shot that failed is failed, not stopped"
    );
}

/// `Always` is for a service whose absence is itself the failure.
#[test]
fn an_always_policy_restarts_even_a_clean_exit_and_never_gives_up() {
    let mut restarts = Restarts::under(RestartPolicy::Always {
        backoff: Backoff {
            initial: Millis(100),
            max: Millis(100),
            multiplier_percent: 100,
            jitter_percent: 0,
        },
    });
    let logs = Capture::detached();
    let at = Instant::now();

    assert_eq!(
        restarts.ended(&exit_with(0), at, &logs),
        Decision::Restart {
            after: Duration::from_millis(100),
            attempt: 1
        },
        "a clean exit is still an absence"
    );

    for _ in 0..50 {
        assert!(matches!(
            restarts.ended(&exit_with(1), at, &logs),
            Decision::Restart { .. }
        ));
    }
}
