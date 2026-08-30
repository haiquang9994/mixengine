//! The first request to a site whose pool is asleep — roadmap task **T72a**, measured.
//!
//! `../features/resource-isolation.md` publishes a number: *"Cold path: first request to a stopped
//! site served in under 1.5 s."* Nothing measured it until this file, and on two systems of three
//! there was nothing to measure: a pool on a Unix socket had no idle probe, so it was never stopped,
//! so no request to it was ever cold. T72a is the probe; this is the budget.
//!
//! # Three pools, one sweep
//!
//! An idle policy is a whole number of minutes, so waiting per round would spend three minutes
//! standing still. One sweep stops all three pools; the three requests are then made one after
//! another and timed separately, and the wait is paid once.
//!
//! **The pools must be stopped by the sweeper and not by a person.** A service somebody stopped is
//! one the activator closes the connection on, deliberately — T70's D8 — because the tool must not
//! overrule its user. So this sets a policy and waits, rather than calling `service stop`.
//!
//! # Three versions rather than three copies
//!
//! `MIXENGINE_PHP_RUNTIMES` names them, and the `bench` job fetches **7.0.33, 7.4.33 and 8.3.33**.
//! The first is the floor this product offers, the second is the legacy version people still run,
//! and the third is what the `test` job pins. **Two of the three predate `pm.status_listen`
//! entirely**, so this is also the standing proof of T72a's decision to read the status page off the
//! pool's own socket: the day somebody reaches for the cleaner arithmetic, two thirds of this
//! measurement go red rather than a paragraph being disbelieved.
//!
//! Three numbers rather than one for a second reason: **a single CI measurement has misled this
//! project before**. The M3 warm-start bench is bimodal on ubuntu, where a red has meant a bad
//! minute rather than a regression. Three admit a median.
//!
//! # What is timed
//!
//! From the request to the last byte of the response, including everything MixEngine does not
//! control — Caddy's dial and its retry, the activator's accept, php-fpm's own boot, PHP compiling
//! the script. That is what the person waiting for the page experiences, and no share of it is
//! excused. **If it goes red, read it as a functional finding before reading it as a slow one.**
//!
//! # Release, ignored
//!
//! `#[ignore]`d because this belongs to the `bench` job rather than to `test`, and the budget is
//! asserted **only in a release build** — `idle_footprint.rs`'s shape exactly. A debug run still
//! measures and still prints.

mod harness;

use std::time::{Duration, Instant};

use harness::frontend::request_as;
use harness::{json, php_site};

/// What the first request to a sleeping site may cost.
///
/// The number `features/resource-isolation.md` publishes, gated here rather than reduced to what
/// this machine happens to manage. If a system cannot meet it, that is written down and raised as a
/// product decision — it is not edited to fit.
const BUDGET: Duration = Duration::from_millis(1500);

/// The shortest policy `service idle` accepts, and therefore the wait this suite pays once.
const AFTER: &str = "1m";

/// How long the sweeper is given to have stopped every pool.
///
/// Generous against the one minute it should take: the sweeper reads every thirty seconds and a
/// policy is spent in whole readings, so a pool crossing the line just after a sweep waits for the
/// next one. A loaded runner adds to both.
const SWEPT: Duration = Duration::from_secs(240);

/// What `mix service status <pool>` says the pool is doing.
fn state(served: &php_site::Served, pool: &str) -> String {
    json(&served.home.mix(&["service", "status", pool, "--json"]))["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// **A request to a site whose pool the sweeper stopped is served inside the published budget.**
#[tokio::test(flavor = "multi_thread")]
#[ignore = "a budget, measured by the bench job — see the module note and ci.yml"]
async fn a_first_request_to_a_sleeping_site_is_served_inside_the_budget() {
    let served = php_site::served(&php_site::runtimes()).await;

    for site in &served.sites {
        let set = json(
            &served
                .home
                .mix(&["service", "idle", &site.pool, "--after", AFTER, "--json"]),
        );
        // **The probe is asserted as well as the duration**, and it is the assertion this whole
        // task turns on: a policy carrying no probe is one `generate` will not attach, so the pool
        // would run for ever and the wait below would time out saying nothing about why.
        //
        // Each system's own, which is the recipe's split: a pool on a socket is asked, and
        // `php-cgi.exe` on a port is counted.
        assert_eq!(
            set["policy"]["probe"]["type"],
            if cfg!(windows) {
                "connections"
            } else {
                "fast_cgi_status"
            },
            "the pool was not given the probe this system measures it with: {set}\n{}",
            served.home.daemon_log()
        );
        assert!(
            !set["policy"]["after"].is_null(),
            "the pool was not given an idle policy: {set}\n{}",
            served.home.daemon_log()
        );
    }

    // --- one sweep, for all three ---------------------------------------------------------------

    let waiting = Instant::now();

    loop {
        let asleep = served
            .sites
            .iter()
            .filter(|site| state(&served, &site.pool) == "stopped")
            .count();

        if asleep == served.sites.len() {
            break;
        }

        assert!(
            waiting.elapsed() < SWEPT,
            "only {asleep} of {} pools were stopped in {:?}; without a sweep there is no cold path \
             to measure\n{}",
            served.sites.len(),
            waiting.elapsed(),
            served.home.daemon_log()
        );

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    println!(
        "cold path: {} pools asleep after {:?}",
        served.sites.len(),
        waiting.elapsed()
    );

    // --- three requests, timed one at a time ----------------------------------------------------

    let mut taken = Vec::with_capacity(served.sites.len());

    for site in &served.sites {
        // **Asserted before the request and not after.** A pool that was already running would make
        // this a warm measurement reported as a cold one, which is the failure that reads as a very
        // good number.
        assert_eq!(
            state(&served, &site.pool),
            "stopped",
            "{} woke up before it was asked to, so this round measures nothing\n{}",
            site.pool,
            served.home.daemon_log()
        );

        let began = Instant::now();
        let answer = request_as(served.port, "/", &site.domain).unwrap_or_else(|| {
            panic!(
                "the front end answered nothing at all for {}\n{}",
                site.domain,
                served.home.daemon_log()
            )
        });
        let took = began.elapsed();

        assert!(
            answer.contains("200"),
            "a sleeping site answered something other than 200: {answer}\n{}",
            served.home.daemon_log()
        );
        assert!(
            answer.contains(&site.says),
            "the body is not what this site's PHP prints, so the pool did not serve it: {answer}\n{}",
            served.home.daemon_log()
        );
        assert_eq!(
            state(&served, &site.pool),
            "running",
            "{} served a request without being started, which cannot happen\n{}",
            site.pool,
            served.home.daemon_log()
        );

        println!("cold path: {} woke and served in {took:?}", site.version);
        taken.push((site.version.clone(), took));
    }

    // --- the numbers, printed whatever happens --------------------------------------------------

    let mut sorted: Vec<Duration> = taken.iter().map(|(_, took)| *took).collect();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];

    println!(
        "cold path: median {median:?} over {} rounds, budget {BUDGET:?} — {}",
        sorted.len(),
        taken
            .iter()
            .map(|(version, took)| format!("{version} {took:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // **Release only.** A debug daemon and a debug `mix` are a different program, and a number
    // measured there is about the profile rather than about the design — `idle_footprint.rs`'s rule,
    // and `overhead.rs`'s before it.
    #[cfg(not(debug_assertions))]
    for (version, took) in &taken {
        assert!(
            *took <= BUDGET,
            "the first request to a site on PHP {version} took {took:?}, over the {BUDGET:?} this \
             project publishes. Read this as a functional finding before reading it as a slow one."
        );
    }
}
