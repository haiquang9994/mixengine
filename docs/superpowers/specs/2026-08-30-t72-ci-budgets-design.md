# T72 — CI budgets: idle footprint and cold path

Roadmap: [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md).
Feature: [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md),
"Measuring, not guessing" and the two numbers under it. Standard:
[.claude/standards/testing.md](../../../.claude/standards/testing.md), "Performance guards".
Predecessors: [T70](2026-08-29-t70-on-demand-activation-design.md), whose activator the cold path
walks through, and [T71](2026-08-30-t71-metrics-history-design.md), whose sampler the footprint is
read from.

## What this is for

Two numbers are published and defended in this project's own documents:

- **Idle footprint** — daemon plus the web server, nothing else running: **under 60 MB RSS**.
- **Cold path** — the first request to a stopped site served in **under 1.5 s**.

Neither is measured anywhere. Both are the kind of number that decays quietly: no single commit
makes a daemon fat, and nobody notices a cold start sliding from 900 ms to 1.4 s until it is 2.
This task turns both into a build that goes red, which is what the two budgets already in the `bench`
job do for the shim's overhead and for M3.

**It is also the first proof that a PHP site is served end to end.** Caddy renders `php_fastcgi` with
the pool's address and its activator's, and `php_fpm.rs` speaks FastCGI straight to a pool, and
`caddy.rs` serves a static string — but nothing in this repository has ever made an HTTP request that
reached PHP through Caddy. The cold path is that request. **If it goes red, read it as a functional
finding before reading it as a slow one.**

## What changed once the code was read — the cold path left this task

**Amended during implementation.** The cold path is **not** built here, and the reason is not
performance. On Linux and macOS a php-fpm pool listens on a Unix socket: `listens_on_tcp()` is
`cfg!(windows)`, so `activation_port_needed` and `activator` both answer nothing, and
`held_while_stopped` is the trait's empty default. On two of three systems a site's pool is never
idle-stopped and nothing would wake it — there is no *stopped site* for a first request to reach.
T69 had already recorded the other half of the same fact and had not drawn out what it costs this
promise.

Measuring it on Windows alone was refused, on this spec's own argument for a single 60 MB across
three systems: a cross-platform promise gated on one system is not gated. It is **T72a**, which gives
such a pool the activator T70a already made possible.

So D4 below describes a suite this task did not build, and D5's PHP fetch was not added. Both are
kept as written rather than deleted, because T72a starts from them.

**And the first thing this task actually did was find two defects in what it was measuring** — see
the roadmap entry: threads counted as processes, and a daemon's row that included every service it
supervises. The first measurement read 1558 MB against a 60 MB budget, and neither fault was
visible until a number somebody had argued for was pointed at the sampler.

## D1 — Two suites, in the shape the two existing budgets already have

`crates/mixengine-cli/tests/idle_footprint.rs` and `crates/mixengine-cli/tests/cold_path.rs`, each
following `warm_start.rs` and `overhead.rs` rather than inventing a third convention:

- **`#[ignore]`d**, so they belong to the `bench` job and never stand between a correctness suite and
  its answer.
- **Gated only in a release build.** A debug daemon is a different program, and a number measured
  there is about the profile rather than about the design. A debug run still measures and still
  prints.
- **A median over rounds**, never a single reading, because a shared runner has bad seconds.
- **One summary line per number**, in `warm_start.rs`'s `[m3]` shape, so a person reading a red job
  sees the measurement and the budget side by side without opening a log.
- **Each round asserts what it measured.** Both numbers have a failure mode that reads as a pass: a
  footprint taken while nothing is running is small, and a cold path timed against a service that was
  already up is fast.

Two files rather than one, because they need different homes: the footprint needs a daemon and Caddy,
and the cold path needs those plus PHP, three pools and three sites.

## D2 — The footprint is read through `metrics.snapshot`, and the set is asserted

The number is the sum of `rss_bytes` over the subjects `metrics.snapshot` reports, and the suite
asserts the subject set is **exactly** `{daemon, service:caddy}`.

**Through the daemon's own sampler rather than a reading of this machine taken beside it**, because
that is the number `mix metrics` shows a user and the one `resource-isolation.md` is about — one
mechanism, one answer. It is also what T71's design said this task would need: a reading it can
compare across three platforms, taken the same way on each.

**The set assertion is the load-bearing half.** A home where Caddy failed to start reports one
subject and a very good number; the budget alone would call that a pass.

**What `rss_bytes` is, said rather than assumed.** One `sysinfo` call on all three systems, which is
one *mechanism* and not quite one *quantity*: on Windows it is the working set, which the OS trims
only under memory pressure, so an idle runner keeps more of it than Linux keeps RSS for the same
program. The budget is a single 60 MB on all three anyway, because that is the number this project
published — and if one system cannot meet it, that is a finding to write down rather than a reason to
grow three numbers out of one promise.

**The reading perturbs what it reads, slightly.** Answering `metrics.snapshot` costs a walk of the
process table — about 10 ms on Windows, measured at T71 — and the daemon is one of the subjects.
Taking the median of several readings a second apart is what that buys off.

## D3 — Settle, and what the measurement is honestly not

Everything but the daemon and Caddy is stopped, the home is left alone for **thirty seconds**, and
then the readings are taken.

**This is not "after 30 idle minutes", and the design does not pretend it is.** The daemon being
measured has just installed packages, rendered configuration and walked a start plan; its RSS carries
the high-water mark of all of that, which an allocator returns to the OS slowly or not at all. A
daemon that has been up for an afternoon holds less.

**Which makes the measurement wrong in the safe direction, and that is why it is acceptable.** The CI
number is *worse* than the one a real idle machine would show, so passing at 60 MB here means passing
comfortably there. A budget that errs strict is a budget doing its job.

**What was rejected: restarting the daemon and letting the next one adopt Caddy.** It would be much
closer to an idle daemon — a fresh process supervising a server that was already running — and it
cannot be done on two of the three systems.
[ADR 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md) is why: a
daemon leaving takes its whole job down on Windows and its immediate children on Linux, so there
would be nothing to adopt on either, and the suite would measure a daemon standing alone. Only macOS leaves the server
running. One measurement that means three different things is worse than one that is honestly
approximate.

## D4 — The cold path: three pools, one sweep

A round is a single `GET` to a site whose pool is stopped, timed from the request to the last byte of
the response, asserting **200**, asserting the body is what the PHP file prints, and asserting the
pool's state was `stopped` before and `running` after.

**The pool must have been stopped by the sweeper and not by a person**, which is T70's D8: a service
a person stopped is one the activator closes the connection on, deliberately, because the tool must
not overrule its user. So the suite sets an idle policy and waits for a sweep.

**Three pools and three sites, stopped by one sweep.** The minimum idle policy is one minute —
`service.set_idle` takes minutes — so waiting per round would spend three minutes of a bench job
standing still to measure three numbers of about a second each. One sweep stops all three; the three
requests are then made one after another and timed separately. The wait is paid once.

**The second and third rounds are also the more realistic ones**, which is a bonus rather than the
reason: on a real machine something else is usually already running when a pool is woken, and only
the first round here has an otherwise quiet home.

**What is timed includes what MixEngine does not control** — Caddy's dial, php-fpm's own start — and
that is the point: it is the number the person waiting for the page experiences. The activator's own
share is not gated. php-fpm's ready check is a connect retried every 50 ms with no fixed settle, so
the budget is not pre-spent by a constant chosen elsewhere; a red result is a real finding.

## D5 — What the `bench` job grows

- One line in the existing "Fetch the three servers M3 is about" step, for PHP, pinned to the same
  version the `test` job pins — for the reason that step already states: a bench comparing itself
  against last month's number has to be measuring the same programs.
- **Two steps, not one**, on the rule every real-server step in this job follows: a failure should
  name what failed without anybody reading the log.
- Both after the release builds of `mixengined` and `mix` that are already there.

No comparison against master, and no per-OS thresholds. The budget is the number, exactly as it is
for the shim's 15 ms and M3's 10 s — the roadmap line says *failing the build on regression*, and a
fixed ceiling is what those two other guards mean by it. Running the bench on `master` as a control
when a red looks like noise stays what it is today: something a person does, and a note in
`.claude/roadmap/`.

## D6 — What this task does not do

- **No new API and no new CLI.** Everything both suites need — `metrics.snapshot`, `service.set_idle`,
  `site.create` — shipped in T69, T70 and T71.
- **No third number.** The README also promises "after 30 idle minutes only `mixengined` and the web
  server are running", which is M7's acceptance criterion and is proved by the idle suite that
  already exists, not here.
- **No trend reporting.** A table of numbers nobody is assigned to read is not a guard.
- **No tuning.** If a budget is missed, this task reports it; making the daemon smaller or the wake
  faster is T73's and its own follow-ups'.

## Testing

The suites *are* the tests, so what needs saying is how each of them fails honestly:

- **The footprint suite fails if Caddy is not running**, by the subject-set assertion, before any
  number is compared.
- **The cold path suite fails if the pool was already running**, by the state assertion taken before
  the request, and fails if the response was not PHP's, by the body assertion.
- **Both fail loudly in a debug build** only by printing; the comparison is release-only, so a
  developer running `cargo test -- --ignored` locally gets numbers rather than a red they cannot act
  on.
- **Both are run once in CI before the gate is trusted**: a first run whose numbers are in the log is
  what tells us whether 60 MB and 1.5 s are met on all three systems today. If one is not, the
  roadmap entry says so and the number stays the published one.

## Documents to update in this task

- [.claude/standards/testing.md](../../../.claude/standards/testing.md) — the two budgets move from a
  bare line into the same shape the other two guards have: which file, which job, what is gated and
  what is only reported.
- [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md) — the
  acceptance criterion "the CI benchmark fails the build if the idle footprint regresses" stops being
  a promise.
- [.claude/operations/build-and-release.md](../../../.claude/operations/build-and-release.md) — the
  `bench` job's description grows the two steps.
- [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md) — tick T72,
  with the measured numbers written down, because the next person to touch either budget needs to
  know what the margin was.

No ADR. Both numbers were argued for before this task; this measures them.

## Order of work

1. `idle_footprint.rs`: the home, the settle, the readings, the subject-set assertion, the summary
   line. Measured and printed, **not yet gated**.
2. `cold_path.rs`: three sites and three pools, one sweep, three timed requests. Measured and
   printed, not yet gated.
3. The `bench` job: the PHP fetch line and the two steps.
4. **One CI run to read the six numbers.** Three systems, two measurements.
5. Turn the gates on at 60 MB and 1.5 s, or — if a system cannot meet one of them — write down what
   it measured and raise it as a product decision rather than editing the number.
6. The four documents, and the roadmap entry with the numbers in it.
