# T71 — Metrics history

Roadmap: [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md).
Feature: [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md),
"Measuring, not guessing". API surface:
[.claude/architecture/daemon-and-ipc.md](../../../.claude/architecture/daemon-and-ipc.md).

## What this is for

Three things wait on one sampler, and that is why it is a task of its own rather than a field on
something else.

A **client draws a graph**: what is eating my battery, and what does MixEngine cost when I am not
using it. That is the promise `resource-isolation.md` makes in as many words, and it is a question
about the past.

**T71a is the macOS memory watchdog** — warn at a `memory_mb` that platform cannot enforce, restart
at a threshold when the service asks to be. macOS has no hard memory cap, so the limit becomes *a
reading taken repeatedly and compared*, which is this sampler and nothing else. T68 deliberately did
not build a second loop to serve one field on one operating system.

**T72 gates a number in CI**: idle footprint under 60 MB RSS. It needs a reading it can compare
across three platforms, taken the same way on each.

## The contradiction this design has to settle first

Two documents in this repository describe the same sampler and do not agree.

[`resource-isolation.md`](../../../.claude/features/resource-isolation.md) promises a 24-hour history
that answers *"what is eating my battery"*. [`client-surface.md`](../../../.claude/features/client-surface.md)
says metrics are *"sampled only while watched"*, because polling a sleeping laptop is the behaviour
these documents criticise elsewhere.

Only-while-watched cannot keep the first promise. A person opens a client at nine in the morning and
asks what drained the battery overnight; nobody was watching overnight, so the honest answer under
that rule is an empty chart. What such a history holds is exactly the minutes somebody was already
looking at the live numbers — the minutes least in need of a recording. And T71a is worse off than
that: a watchdog that only watches RAM while a client is open is not a watchdog.

**So there are two rates** (D1). The history is kept by a slow sampler that always runs; the fast one
exists only while somebody is watching. `client-surface.md` is corrected rather than satisfied, and
the correction is written into it.

## D1 — Two rates, one loop

One task in the daemon. Its period is **60 s** by default and **1 s** while at least one client holds
a `GET /metrics` stream open. The last stream closing puts it back to 60 s.

**One loop and not two.** Two loops — a slow one for history, a fast one for the stream — would
measure the same processes at two different moments and hand a client two answers to one question,
and while somebody watched, the machine would be measured twice for every minute stored.

The rate change is delivered by a `tokio::sync::watch<bool>` the loop selects on, not by a flag it
reads at the top of each iteration. A client opening the stream must get its first frame at once; a
flag read once per tick would make it wait out the rest of a 60-second sleep first.

**The count of watchers is the count of open connections**, held by the API layer and nothing else.
That is what makes "sampled only while watched" a property of the mechanism rather than of anyone's
care: a client that dies without saying goodbye closes its socket, and a socket closing is the
unsubscribe. This is the reason the stream is a stream and not an event (D6).

**The obligation this decision creates, paid before the default is fixed.** The cost of one reading
is asserted nowhere in this document. Step 1 of the order of work measures it on Windows and on
Linux, and the number goes into the doc comment beside `DEFAULT_IDLE_SAMPLE_SECONDS`. If it turns
out dearer than expected the default period widens; the minute grain (D4) does not change, and a
minute with no row is already a legal thing (D5). A repository that criticises polling by name owes a
number here rather than an argument.

## D2 — What a sample is, and what it is not

```rust
pub struct MetricsFrame {
    pub at: Timestamp,
    pub samples: Vec<MetricsSample>,
}

pub struct MetricsSample {
    pub subject: MetricsSubject,      // Daemon | Service(ServiceId)
    pub cpu_percent: Option<f32>,
    pub rss_bytes: u64,
    pub processes: u32,
}
```

**The moment belongs to the frame and not to the sample.** Every subject in one frame was measured in
one refresh, so a timestamp per sample would be the same value repeated once per service — and a
value repeated is a value free to disagree with itself the day somebody assembles a frame from two
readings.

**`cpu_percent` is a percentage of one core**, so 250 is two and a half cores' worth. That is the
same unit `ResourceLimits::cpu_percent` is declared in, deliberately: a client that offers a cap and
then draws the usage must not have to convert between the two, and a number that meant something
different in each place would be discovered by whoever first drew them on one axis.

**`Option`, and never a zero standing in for a refusal.** CPU is a difference between two readings,
so the first measurement after the daemon starts — and the first after a service starts — has nothing
to subtract from. That reading has no CPU value. Reporting `0.0` there would draw an idle service
during the one second it is most expensive, which is exactly T69's `Unmeasurable` rule: *could not
measure* and *measured nothing* are different facts and may not share a representation.

**`rss_bytes` is the sum over the group, and shared pages are counted once per process.** A php-fpm
master and its four workers share most of what they map; adding their resident sets overstates the
group. There is no cross-platform way to do better — PSS exists only on Linux — so the number is an
overestimate, it is named as one, and it errs in the safe direction: MixEngine looks dearer than it
is, which is the correct way to be wrong about a number defended in a README.

**`rss_bytes` is not the quantity a `memory_mb` limit is judged against**, and the field's
documentation says so. A limit is enforced on commit charge on Windows and on charged pages on
Linux — see `MemoryMeasure` in `mixengine-proto`'s `limits` module. A client that renders
"480 MB / 512 MB" out of this field and a limit is rendering two different measurements as one ratio.
The doc comment points at `MemoryMeasure` so that whoever writes that widget reads this first.

**`processes` costs nothing and answers a real question** — the group is walked anyway, and "how many
workers does this pool have right now" is otherwise unanswerable from the API.

## D3 — The reading lives in `mixengine-platform`, and it is asked for by pid *and* start time

A new capability beside `ConnectionCount`, which it resembles in shape and in purpose — a number
asked of the machine, on a schedule, for as long as the daemon runs:

```rust
pub trait ProcessMetrics: std::fmt::Debug + Send + Sync {
    /// One reading per group root that is still alive. A root that is not is absent.
    fn measure(&self, roots: &[GroupRoot]) -> Vec<GroupReading>;
}

pub struct GroupRoot { pub pid: u32, pub started: StartTime }
pub struct GroupReading { pub pid: u32, pub cpu_percent: Option<f32>, pub rss_bytes: u64, pub processes: u32 }
```

`resource-isolation.md` says "`sysinfo` in the daemon", and this moves it one crate down. Two reasons,
and the second is the larger:

The workspace rule is that no crate above this one contains an OS call, and every capability that
asks the machine something already lives here behind a trait with a `mock` beside it.

**And the mock is what makes T71 testable at all on a developer's machine.** The hard part of this
task is arithmetic over a series of numbers — minute rollover, retention, the rate change. With a
programmable `ProcessMetrics` those are unit tests over invented readings that run in milliseconds.
Without one, every test of the history has to grow a real process and wait for real time to pass.

**The real implementation holds a `Mutex<sysinfo::System>`,** because the CPU figure is a delta and
the state it is a delta from has to survive between calls. One refresh per tick serves every root:
the parent map is built once and each group is walked from its root.

**A root is a pid *and* the moment it was born.** Asking by pid alone is a bug waiting for a busy
machine: a service that exits between two ticks frees a pid the OS may hand to something else, and
the next reading would draw a stranger's memory on MariaDB's chart. The machinery to prevent it is
already here — `services.pid_start_time` and `process::started_at`, built for T18's adoption, answer
exactly this question. A pid whose start time does not match is not the process we meant, so it is
absent from the result, and absent already means *not measured* (D5).

**The daemon measures itself** through the same call, with `std::process::id()` as a root of its own.
`MetricsSubject` is a closed enum — `Daemon` or `Service(ServiceId)` — so the subject of a reading
stays a value a client can match on rather than a string it has to recognise.

**A total is not published.** Adding the subjects up is an addition a client can do; publishing it
here would be a second number describing the same facts, free to drift from its own parts.

## D4 — The minute row, and the accumulator that fills it

Migration `0011_metrics.sql`:

```sql
metrics_minutes(subject, minute, cpu_avg, cpu_peak, rss_avg, rss_peak, samples)
   -- subject: 'daemon', or 'service:' + a ServiceId
   -- minute:  epoch milliseconds truncated to the minute — an INTEGER, on
   --          services.last_started_at's rule: compared, never displayed
   -- samples: how many readings the row is made of — 1 when nobody watched,
   --          up to 60 when somebody did
   -- PRIMARY KEY (subject, minute)
```

**The `service:` prefix is not decoration.** `ServiceId::parse` accepts a bare `name` with no
instance, so `daemon` is a legal service id, and a column where the daemon's own rows are spelled
`daemon` would hand a service by that name somebody else's history. A `:` cannot occur in a service
id at all — the grammar allows letters, digits, `-`, `@` and `.` — so the prefix makes the two spaces
disjoint by the same rule that validates the ids.

The minute being assembled lives in memory; when the clock crosses into the next minute the finished
row is written. Nothing is read-modify-written in SQL, a daemon that dies loses at most the partial
minute, and a subject that is not running contributes no row at all.

**`samples` is on the row because an average of one reading and an average of sixty are not the same
kind of number.** A client may draw both, but it may not draw them as though they were equally
supported, and the row has to carry the difference for that to be possible.

**`cpu_peak` and `rss_peak` are the maximum of the readings the row is made of** — not of the minute.
When `samples` is 1 they equal the averages, and that is what the pair of columns means: the largest
thing seen, out of however many times we looked. They exist because "what ate my battery" is a
question about spikes: a service that holds 900 MB for two seconds and 200 MB for the rest of the
minute leaves nothing behind in an average.

**No foreign key to `services`, on purpose.** A service deleted at two in the morning is still the
answer to what happened at two in the morning, and a cascade would delete precisely the evidence
somebody came looking for. The 24-hour trim is what bounds the table, and it does not care whether
the subject still exists. This is the one table here whose rows outlive their subject.

**Retention is a `DELETE` of everything older than 24 hours, run after each minute is written**, on
`events`' precedent — ring-trimmed, stated in `data-model.md`, not left to a policy invented later.
The comparison is against a wall clock (`SystemTime`), never against elapsed `Instant`s: a laptop
that slept eight hours has to trim eight hours of rows on the tick after it wakes, and tokio's clock
counted none of that time.

## D5 — A missing minute means *not measured*

A minute with no row for a subject is not a minute in which that subject used nothing. It is a minute
in which nothing was measured: the service was stopped, or the machine was asleep, or the daemon was
being replaced by an update.

This is the rule T69 established for idle observations and the reason its sweeper counts sweeps
rather than comparing timestamps: tokio measures from `Instant`, which counts no time while a laptop
is suspended. A closed lid produces an eight-hour gap in this history and that is the correct
recording — nobody was watching *and nothing was running to watch*.

The consequence for whoever draws it: **a gap is drawn as a gap.** Joining the point before a gap to
the point after it with a straight line invents a night's worth of measurements that were never
taken. Nothing in the API can enforce that, so the API's job is to make the gap unmistakable, which
it is — there is no row, rather than a row of zeroes.

## D6 — One stream, two methods, and a retired event

**`GET /metrics`** — Server-Sent Events, one `MetricsFrame` per tick carrying every subject measured
on that tick. Opening the connection is the subscription; closing it is the end of it. Back-pressure
is per connection, exactly as `GET /logs/{id}` has it: a slow reader slows its own stream and nobody
else's.

**`metrics.snapshot`** — one `MetricsFrame`, every subject, as of now. The same type the stream
carries, because it is the same thing: a client that renders a frame renders both. **It takes a reading rather than serving the last
one.** Without that, the plain `mix metrics` — the command people will type most — prints numbers up
to a minute old, and a service started ten seconds ago does not appear at all. A reading taken for
somebody who is waiting for it is the cost landing in the right place. A reading younger than one
second is reused instead, so a script calling this in a loop cannot drive the sampler at a rate it
never opened a stream to ask for.

**`metrics.history { subject?, since, until? }`** — the minute rows, capped at 24 hours because that
is all there is. `snapshot` is *now*, `history` is *just now*, the stream is *from here on*; one name
per tense, and none of them returns two shapes depending on its arguments.

**`DaemonEvent::MetricsSample` is retired**, and `daemon-and-ipc.md` loses the variant it declares
today. The event bus is a bounded broadcast of 1024 messages, shared by every client, sized for state
changes; ten services at one sample a second would spend a client's entire allowance in a hundred
seconds and evict exactly the `ServiceStateChanged` it opened the stream for. It would also leave the
daemon with no way to know when to stop measuring — a subscription would need explicit
`metrics.subscribe`/`unsubscribe` calls, and a client that crashes without the second one leaves the
daemon sampling every second forever, which is the behaviour the whole design is arranged to avoid.

**No new ADR.** [ADR 0009](../../../.claude/decisions/0009-logs-travel-on-their-own-stream.md) already
makes this argument about log lines; a second record making it again about metrics would be two
descriptions of one decision, which is the thing this codebase refuses everywhere else. The variant
is removed from `daemon-and-ipc.md` with the reason written beside it and a pointer to 0009 — the
same treatment the `LogLine` variant already received in that document.

## D7 — The CLI

`mix metrics` prints one table and exits. `--watch` opens the stream and prints each tick beneath the
last, scrolling, the way `mix service logs -f` does. `--since 1h` reads history. `--service <id>`
narrows the subject. `--json` is what scripts and T72 read.

**Not a full-screen `top`.** Redrawing in place is a capability this CLI has never had, it needs a
separate path for output that is not a terminal, and it cannot be asserted against in a test. A
scrolling table pipes into a file, runs in CI, and is the shape T72 needs from the same command.

`--watch` exists in this task rather than a later one for a reason beyond convenience: without a
client that opens the stream, nothing in this repository exercises the 1-second rate, and the rule
that no capability may be reachable only from a client this repository does not ship would be broken
on the day it was written.

## D8 — Configuration

A `[metrics]` section beside `[services]`:

| key | default | why it is a key at all |
| --- | --- | --- |
| `sample_seconds` | 1 | the rate while watched; a test cannot wait a minute to see two frames |
| `idle_sample_seconds` | 60 | the background rate — the number D1's measurement fixes |
| `retention_hours` | 24 | no test can wait a day to watch the trim happen |

Each is here because a loop no test can move is a loop nothing exercises — the reason
`idle_check_seconds` is a key, stated in its own doc comment. Zero is refused for the two periods, on
`idle_check`'s precedent: it is not a short interval, it is a loop with no pause in it.

## D9 — What this task does not do

**T71a** is the macOS memory watchdog. It reads this sampler and compares; nothing here warns,
restarts, or knows what a threshold is. `LimitSupport` keeps answering `Unsupported` for memory on
macOS until it lands.

**T72** is the CI budget. This task publishes a number and gives `mix metrics --json` a shape to
publish it in; whether that number is under 60 MB is gated there.

**Reading a Windows Job Object was considered and refused.** Every supervised service already runs in
a Job, and `QueryInformationJobObject` reports the job's total CPU time and peak memory directly —
more accurate than walking a pid tree and immune to a process that reparents itself. It is refused
because it would measure Windows by a different mechanism than the other two systems, at the moment
T72 is about to hold all three to one threshold: three numbers that cannot be compared are worse than
three numbers that are wrong in the same direction. The pid walk overstates shared pages identically
everywhere. If a service that detaches from its parent ever ships, this is the answer waiting.

## Testing

**In `mixengine-platform`**, against the mock: a group's readings are summed over the tree; a root
whose start time does not match is absent from the result; a root that has exited is absent.

**In the daemon**, over a programmed `ProcessMetrics`: sixty one-second readings collapse into one
row with `samples = 60` and a peak that is not the average; one reading collapses into a row with
`samples = 1`; the row is written when the minute rolls and not before; a subject that stops
producing readings stops producing rows rather than producing zeroes; the trim deletes what is older
than `retention_hours` and keeps what is not, measured against a wall clock the test moves.

**The rate change**: with no watcher the loop measures at the idle period; opening the stream makes
the next reading arrive without waiting out the current sleep; closing the last stream returns it to
the idle period. This is the assertion that keeps "sampled only while watched" true.

**End to end, with `fakeservice`**: `GET /metrics` delivers frames naming a running service; the CPU
field of the very first reading is `null` rather than `0`; `metrics.history` reads back a minute that
the stream had already reported; `mix metrics --json` parses.

## Documents to update in this task

- `.claude/architecture/daemon-and-ipc.md` — `metrics.history` added to the namespace table,
  `MetricsSample` removed from the event enum with the reason and a pointer to ADR 0009.
- `.claude/architecture/data-model.md` — the `metrics_minutes` table, its lack of a foreign key, and
  its trim.
- `.claude/features/client-surface.md` — "sampled only while watched" corrected to the two rates, and
  why the history could not be kept under the old rule.
- `.claude/features/resource-isolation.md` — the sampler is in the platform layer, not the daemon;
  what `rss_bytes` overstates and what it is not.
- `.claude/architecture/platform-abstraction.md` — `ProcessMetrics` in the trait list.
- `.claude/roadmap/phase-7-efficiency.md` — T71 ticked, with what it did not do and who owns that.

## Order of work

1. **Measure one refresh**, on Windows and under WSL, and fix `idle_sample_seconds`' default against
   the number. It goes in the doc comment. Nothing else waits on this, but the default does.
2. **`ProcessMetrics` in `mixengine-platform`** — trait, `sysinfo` implementation, mock, and the
   start-time check. Testable on its own.
3. **The proto types** — `MetricsSample`, `MetricsSubject`, the history row, the three method
   payloads.
4. **The store** — migration, the minute accumulator, the trim. Unit-tested over invented readings
   with no loop and no clock of its own.
5. **The loop** — the two rates and the watcher signal.
6. **The API** — `GET /metrics`, `metrics.snapshot`, `metrics.history`.
7. **The CLI**, and the end-to-end suite.
