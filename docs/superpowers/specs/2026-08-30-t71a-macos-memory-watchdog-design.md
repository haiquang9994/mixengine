# T71a — The macOS memory watchdog

Roadmap: [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md).
Feature: [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md),
"Hard limits" and "Measuring, not guessing". Predecessors:
[T68](2026-08-26-t68-resource-limits-design.md), which deferred this, and
[T71](2026-08-30-t71-metrics-history-design.md), whose sampler is the whole of what it needs.

## What this is for

`ResourceLimits::memory_mb` is a wall on Windows and on Linux, and on macOS it is a number the
daemon stores and nothing reads. T68 said so in as many words and answered `Unsupported` rather than
pretending otherwise. This task is the one part of that promise that is not a call on a kernel
object: macOS has no per-process memory ceiling, so the limit becomes **a reading taken repeatedly
and compared** — warn while the service is over, restart it if its recipe says that is safe.

T68 refused to build the sampler for it, because a second loop inside the supervisor would have been
replaced by T71 three tasks later. T71 has landed; `Host::process_metrics` and the minute rows are
here. What is left is the comparing.

## The trap this design has to avoid first

A watchdog is easy to write and easy to write *badly*, and the two failures are opposites:

- **Too eager**, and a service is killed for one instantaneous reading taken while it was doing its
  job — a php-fpm pool restarted mid-request because somebody uploaded a large file.
- **Too willing**, and a `memory_mb` set below what the service needs at boot turns into a machine
  that restarts a database every three minutes for as long as the daemon runs.

Both are avoided by the same idea, and it is T69's rather than a new one: **the arithmetic is in
observations, never in elapsed time**, and an action is taken only on evidence that repeated itself.

## D1 — A consumer of finished minutes, not a second loop and not part of the first

The watchdog is a task in `mixengine-daemon/src/services/watchdog.rs`. The sampler publishes each
`MetricsMinute` it finishes; the watchdog receives them, decides, and acts through the registry.

**Not inside `Sampler::take`**, although the minutes are assembled there. The sampler would then have
to hold an `Arc<Registry>`, and a restart is a stop-plan and a start-plan — seconds of awaiting
between two readings of a loop whose whole contract is that it measures on a period. A measuring loop
that stops to control something is no longer measuring.

**Not a loop of its own either**, which is what T71 argued against and this respects: the watchdog has
no clock. It wakes when a minute is finished, so its period *is* the sampler's, and there is one
place on this machine that reads the process table.

**Not the `Sweeper` of T69**, which has the registry and a clock already. It skips every service with
no idle policy, and folding memory into it would spend one config key on two unrelated policies.

The decision itself is a pure module beside the task — a `Tally` over subjects, taking minutes and
answering a `Verdict`, testable from invented numbers exactly as `services::idle` is.

## D2 — What arms it: the machine's answer, never the operating system's name

A service is watched when both hold:

- its stored `ResourceLimits::memory_mb` is `Some`, and
- `host.limits().support().memory` is **not** `Enforcement::Hard`.

So the watchdog covers macOS, which has no mechanism, *and* the Linux machine whose session was
never delegated the `memory` controller — where `memory_mb` is today an equally dead number. It
never runs where the kernel is already enforcing, because two things judging one ceiling by two
different quantities would disagree in public.

There is no `cfg!(target_os)` anywhere in this, per `CLAUDE.md`: the daemon asks the platform layer
what this machine will do and believes the answer.

## D3 — The judged quantity is the minute's `rss_avg`, and what that is worth is stated

A finished `MetricsMinute` carries `rss_avg`, `rss_peak` and `samples`. The watchdog compares
**`rss_avg`** against `memory_mb`.

**`rss_avg` rather than `rss_peak`**, because a service that touches twice its usual size for five
seconds a minute is a service doing its work, and the ceiling is about what it *holds*.

**And here is what that is honestly worth.** At the idle rate a minute contains exactly one sample,
so `rss_avg` is that single reading and averaging smooths nothing; only while a client holds
`GET /metrics` open does a minute hold sixty readings and the average become one. **The watchdog is
therefore differently sensitive depending on whether somebody is watching** — the same reading either
way, but a spike survives into the average when nobody is looking and is diluted when somebody is.
This is written down rather than hidden because it is the one place where T71's two rates leak into a
decision. It is tolerable for exactly one reason: what protects against a transient here is not the
average but **three consecutive minutes**, and that rule is the same at either rate.

`rss_bytes` overstates shared pages, identically on all three systems, which is the safe direction
for a threshold that triggers a restart — it fires early, never late. `MemoryMeasure::Resident` is
what says so on the wire (D6).

## D4 — Warn is a state, and the runner keeps sole ownership of the edge

Warning is `ServiceState::Degraded` with `StateReason::OverMemory { rss_bytes, limit_mb }`. It is
persisted, it rides `ServiceStateChanged`, and `mix service list` shows it with no new mechanism
anywhere.

**But the watchdog does not write it.** The runner's health loop owns `Running ↔ Degraded` today —
[`runner.rs`](../../../crates/mixengine-daemon/src/services/runner.rs) moves to `Degraded/Unhealthy`
on a failed probe and back to `Running/Healthy` on a recovery, from the health verdict alone. A second
writer would be a bug on both sides: the next healthy probe would clear an over-memory warning while
the service was still over its ceiling, and a watchdog seeing memory drop would erase a genuine
`Unhealthy`.

So the watchdog sets a verdict and the runner folds:

- a per-entry `over_memory: watch::Sender<Option<OverMemory>>` in the registry, the third channel
  beside `stopping_because` and `asked_to_reload`, set and cleared by the watchdog;
- the runner's loop selects on it as it selects on its probe, and computes one `(state, reason)` from
  both inputs:

| health | memory | state | reason |
| --- | --- | --- | --- |
| healthy | under | `Running` | `Healthy` |
| healthy | over | `Degraded` | `OverMemory` |
| unhealthy | under | `Degraded` | `Unhealthy` |
| unhealthy | over | `Degraded` | `Unhealthy` |

Illness is reported ahead of size when both hold, because it is the more urgent sentence to put in
front of a person: a service failing its probe needs attention whatever it weighs. `Running` is
reached only when both inputs are clear.

**A service that is both is still restarted at the third minute, with `OverMemory` on that
transition** — deliberately, and it is the one place this design says two different things about one
episode. The alternative is a watchdog that stops working on sick services, which is exactly when a
leak is most likely to be the cause.

Two things the runner's shape forces, both found by reading it rather than by designing:

**The fold is evaluated before the health guard, not inside it.** The loop reaches its health verdict
behind `let Some(watching) = health.as_mut() else { continue; }`, so a service whose recipe declares
no `HealthCheck` never arrives there. A fold living inside that branch would watch such a service and
never warn about it.

**Only a change of *state* is written, never a change of reason alone.** `ServiceState::can_become`
has no self-loops, so a second move to `Degraded` is an `IllegalTransition` that `record` logs an
`error!` for — once a minute, for as long as the service is over. The cost is a reason that can lag:
a service that recovers its health while still over its ceiling keeps reading `unhealthy`, although
what is now wrong with it is its size. The alternative is publishing a `Running` it never was, to
every client watching, in order to correct one word.

## D5 — Restart is the recipe's permission, and one per episode

**The recipe decides, not the person.** `ServiceSpec` grows `restart_over_memory: bool`, set from
`Recipe::restart_over_memory_default()` in the shape of `Recipe::idle_default()`. Whether a program
can be restarted under memory pressure without losing something is a property of the program:

- **php-fpm pools: true.** A restarted pool loses in-flight requests, which its own reload already
  risks, and a leaking pool is the case this whole task exists for.
- **mariadb, mysql, postgres: false.** A restart mid-transaction is a data question, and no daemon
  should answer it on a reading.
- **redis, memcached: false.** Restarting a cache is deleting data somebody believes is still there.
- **caddy, nginx, everything else: false**, because `false` is the default and a recipe opts in.

It needs no column: nothing about it is per-home. A person's control over this is `memory_mb` itself —
the watchdog is dormant on every service until somebody sets one.

**One restart per episode.** After restarting, the service must be observed **below** the line for at
least one minute before it may be restarted again. A pool that leaks up to its ceiling every twenty
minutes is therefore rescued every twenty minutes; a `memory_mb` set below what the service needs at
boot produces exactly one restart, then a service sitting in `Degraded/OverMemory` and one
`daemon.log` line saying the watchdog has stopped acting on it. That is the right ending, because that
case is a misconfiguration and not a leak, and a machine restarting a database forever is worse than
a number nobody enforced.

**A missing minute resets the count**, on T69's rule and for its reason. No row for a subject means
*nobody measured* — the service was stopped, the daemon was replaced, the laptop was shut. A machine
that slept eight hours wakes with a count of zero, not with eight hours of evidence.

The restart itself reuses `service.restart`'s walk rather than a second one: the reason is set through
`stopping_because` first, exactly as the idle sweeper sets `StateReason::Idle`, and the stop plan and
start plan are the same two the RPC builds. That walk takes dependents down and brings them back;
no recipe declares `depends_on` today, so the set is empty in this build, and the day T77 fills it an
automatic restart inherits the same blast radius as the manual one rather than inventing a narrower
one nobody specified.

## D6 — What the wire says, and one variant that was overdue

Three additions to `mixengine-proto`, and one attribute:

**`Enforcement::Advisory { why: Option<String> }`.** "This machine watches this field and will act,
but it is not a wall." `why` preserves the distinction T68 made two variants for, moved into an
`Option`: `Some` is a machine that could be fixed and the sentence `mix doctor` prints for it — a
Linux session with no `memory` delegation — and `None` is an operating system with nothing to fix,
which is macOS. Doctor's rule from T68's D6 is unchanged in effect: it prints the `Some` and stays
silent about the `None`.

**`MemoryMeasure::Resident`.** macOS reports `ChargedPages` today for a quantity it does not measure.
The watchdog judges RSS, so `Resident` is the name for it, and `memory_measure` becomes a function of
what is actually judging: `Hard` on Linux means `ChargedPages`, `Advisory` there means `Resident`.

**`StateReason::OverMemory { rss_bytes, limit_mb }`.** The enum is already `#[non_exhaustive]` and
this is the growth it was made for.

**`#[non_exhaustive]` on `Enforcement` and `MemoryMeasure`.** Both are read by a client in another
repository and neither carries it. This is the first release that adds a variant to either, so it is
the release to say that more may come — a matching change, not an unrelated one.

## D7 — Where the per-service truth is reported

`LimitSupport` answers for the *machine* and takes no service, so it cannot say whether a particular
service would be restarted. That belongs to the service's own report:

```rust
pub struct MemoryWatchdog {
    /// Consecutive minutes over the line before the service is restarted.
    pub after_minutes: u32,
    /// Whether a restart follows at all, which is the recipe's answer.
    pub restarts: bool,
}
```

`ServiceLimitsReport` grows `watchdog: Option<MemoryWatchdog>`. `None` means nothing is watching:
either the machine enforces `memory_mb` itself, or this service has not declared one. This follows
`IdleReport`, which carries the policy *and* what would override it rather than collapsing four
different silences into one `Option`.

## D8 — The CLI

`mix service limits <service>` prints one more line when `watchdog` is `Some`, naming both numbers and
what happens at the end of them — "watched: restarted after 3 minutes over 512 MB", or "watched:
warned only, this service is not restarted automatically". No new command: the watchdog is not a thing
a person switches on, it is what `memory_mb` means on this machine, and it belongs in the report that
already answers what `memory_mb` means here.

`mix service list` needs nothing: the warning is a state, and states are already rendered with their
reason.

## D9 — Configuration

One key, `[services] memory_over_minutes`, default **3**, refusing zero on `idle_check_seconds`'
reasoning — zero would restart a service on its first finished minute, which is one reading at the
idle rate.

Three minutes because it is short enough to catch a leak before a laptop starts swapping and long
enough that a service is never restarted on a single observation. It is a key at all for the reason
`idle_check_seconds` is one: the daemon's own suite has to watch the count run out, and a constant no
test can move would leave that path unexercised.

## D10 — What this task deliberately does not do

- **No new column and no history of warnings.** The counts and the re-arm live in the task, like
  `idle::Tally`. A daemon that restarts forgets, which is correct — a service just adopted has been
  observed zero times.
- **No CPU watchdog.** macOS cannot cap CPU either, and a rate has no equivalent of "it is holding too
  much": a service at 100% for three minutes may be doing exactly what was asked of it. `cpu` stays
  `Unsupported` there and means it.
- **No user override of `restart_over_memory`.** The recipe's answer is the answer. Nothing is
  persisted, so the day somebody wants one it arrives as a column whose `NULL` means "what the recipe
  says" — the three-state shape T69 had to buy in advance is free here.
- **No CI budget.** The idle-footprint and cold-path numbers are T72.

## Testing

**Unit, `services::watchdog`** — invented minutes against a `Tally`, the shape `services::idle`'s
tests take:

- three consecutive minutes over the line produce `Restart`; two produce `Warn`
- one minute under resets the count to zero
- a *missing* minute for that subject resets it too
- after a restart, minutes over the line produce no second restart until one minute under is seen
- a spec with `restart_over_memory: false` never leaves `Warn`, however many minutes pass

**Unit, the fold** — the four-row table of D4 as a table test, plus the regression that names the bug
this design was rewritten to avoid: with `over_memory` set, a healthy probe leaves the service in
`Degraded/OverMemory` rather than moving it to `Running`.

**Unit, arming** — a service with `memory_mb` on a mock host reporting `Hard` is not watched; one
with no `memory_mb` is not watched; one with `memory_mb` on a mock reporting `Advisory` is.

**Integration, in the daemon** — the mock host's programmable readings grown past a declared ceiling
across successive minutes, driving the real sampler and the real registry: assert the persisted
transitions, the events, and that the service was stopped and started once and only once.

**Platform** — `crates/mixengine-platform/tests/limits.rs` gains the per-OS assertion that macOS now
answers `Advisory { why: None }` with `MemoryMeasure::Resident`, and that Windows and a delegated
Linux still answer `Hard`.

**(P)**: Windows runs here, Linux runs in WSL, and macOS is `cargo check` only on this machine — its
run is CI's `test (macos-latest)` leg. The design keeps that exposure to two constants in
`macos/limits.rs`; everything that decides anything is OS-independent and exercised against the mock.

## Documents to update in this task

- [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md) — the
  macOS row stops describing a watchdog in the future tense; the judged quantity, the three minutes
  and the one-restart-per-episode rule are stated where the table is.
- [.claude/features/client-surface.md](../../../.claude/features/client-surface.md) — how a graphical
  client draws an `Advisory` memory control differently from a `Hard` one.
- [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md) — tick
  T71a, with what the implementation found that this design did not.

No ADR. The watchdog was argued for in `resource-isolation.md` before T68 deferred it; this builds
what was already decided rather than deciding something new.

## Order of work

1. `mixengine-proto`: `Advisory`, `Resident`, `OverMemory`, `MemoryWatchdog`, the two
   `#[non_exhaustive]`s, `ServiceSpec::restart_over_memory`.
2. `mixengine-platform`: macOS answers `Advisory { why: None }` / `Resident`; Linux answers
   `Advisory { why: Some(..) }` where it answers `Unavailable` today. Platform tests.
3. `Recipe::restart_over_memory_default` and the one recipe that returns `true`.
4. `services::watchdog` — the pure `Tally` and its tests, before anything is wired.
5. The registry's `over_memory` channel and the runner's fold, with the regression test.
6. The sampler publishes finished minutes; the watchdog task consumes them; `main.rs` wires it beside
   the sampler.
7. The restart path, extracted from `Api::service_restart` so both callers share one walk.
8. `ServiceLimitsReport.watchdog`, the RPC, and the CLI line.
9. Integration test, documents, roadmap.
