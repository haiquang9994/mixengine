# M3 — three services healthy in under ten seconds, measured

**Roadmap:** M3, `.claude/roadmap/phase-3-services.md` — the milestone Phase 3 closed without
claiming, listed as an open debt in `.claude/roadmap/todo.md`
**Depends on:** T31 (Caddy), T33 (MariaDB), T35 (Redis), T31a (`package.*`, `service.create`), T29
(the `bench` job, and the shape a budget takes in this workspace)

## What this closes

`.claude/features/services.md` promises a number:

> Fresh install → `mix service start caddy mariadb redis` → all three healthy in under 10 s on a
> warm cache.

Every recipe the sentence names is in and has a suite of its own against the real program. Nothing
starts all three together, and nothing has ever timed it. Phase 3 closed with fifteen of fifteen
tasks done and its milestone unclaimed for exactly that reason — a promise nothing measures is a
design decision nothing is holding to account, which is T29's sentence about the shim and is as true
here.

M3 is the measurement, and the test that keeps it.

## What already exists, and is reused unchanged

- **The three recipes and their real-program suites** — `caddy.rs` (through `harness/frontend.rs`),
  `mariadb.rs`, `redis.rs`. Each fetches an archive `mixengine-packages` published, packs it as a
  `FakePackage`, serves it from a `MockRegistry` that signs its own index, and installs it through
  the shipped `package.install`. This suite does that three times into **one** home.
- **`mix service start` with no service named**, which is "every declared service" walked in
  dependency order — the `Target` in `cli/src/main.rs`. The milestone's sentence names three
  services in one command line and the CLI takes one, so the command the milestone is really about
  is the one that starts the set.
- **`mix` waits by default**: the client returns once the daemon has *walked* the plan, not once it
  has accepted it. So the wall clock of one `mix service start --json` is the number the promise is
  about, with no polling of our own between the start and the answer.
- **`running` is health, not an accept** (T33's finding): MariaDB's ready check is an authenticated
  `mariadb-admin ping`, Caddy's is its admin endpoint, Redis's is a `PING`. A service that reached
  `running` answered its own program's question, so "all three healthy" needs no fourth probe.
- **The `bench` job** (T29): release profile, `#[ignore]`d tests, `--nocapture` because the numbers
  are the output, and a budget asserted only in a release build.

## Decisions

### D1 — The measured command is one `mix service start`, with nothing named

Three sequential `mix service start <id>` calls would measure three walks and add two client process
creations, and would report a number no user path produces. `mix service start` with no target is
the transitive set — which is what `mix service start caddy mariadb redis` means and what a client
would call.

### D2 — "Warm" is defined here, and it is not "fresh install"

The feature document's sentence says both *fresh install* and *warm cache*, and on a real machine
those are different runs. A fresh install has an empty data directory, so the first start is
MariaDB's first-run ritual: `mariadb-install-db` builds a system schema, generates a root password,
puts it in the OS credential store. That is tens of seconds of work by design and no budget of ten
seconds was ever about it.

So this suite measures **two** numbers and gates **one**:

- **The first start**, from an empty home — install, create, start, bootstrap included. Reported,
  gated on nothing. It is the number a person meets once.
- **A warm start**: the same three services, already installed, already bootstrapped, already
  started and stopped at least once, so the binaries are in the file cache and the data directory is
  built. Measured over several rounds, and its median is what the ten seconds is about.

Whichever way the first number lands, `.claude/features/services.md` gets the distinction written
into it — one promise, two runs, said out loud — the way T43 corrected that document's `Degraded`
sentence rather than leaving a claim the code does not make.

### D3 — The median of an odd number of rounds, with the first thrown away

T29's shape: `RUNS` rounds kept, `WARMUP` discarded, an odd count so the median is a measurement
rather than the average of two. A round is `start` (timed) → assert all three `running` → `stop`
(untimed). Five rounds rather than thirty-one: a round here starts three real servers and stops
them, where a shim round is one process.

### D4 — The budget is asserted in release and printed everywhere

A debug daemon is a different program — a debug SQLite, a debug supervisor loop — and a number
measured there is about the profile rather than about the design. A debug run still measures and
still prints, so nothing here can rot unnoticed. Same split as `shim/tests/overhead.rs`.

### D5 — The failure this file would otherwise have is a pass

Three services that never started are faster than three that did, so every round asserts what it
timed: all three `running` after the start, all three `stopped` after the stop, and the MariaDB
first-run job having run exactly once across the whole suite. A home where the install failed, a
service that was never created, a `--no-wait` slipping into the arguments would each make the number
better and are each a failure here.

### D6 — It runs on all three systems, in the `bench` job

The gate is the same everywhere and what it runs over is not: process creation, Defender, and the
cost of a `Command` ready check differ per OS, and this is the only place that difference is written
down as a number.

Linux needs one thing the other two do not, and it is T33's: a secret service. The MariaDB bootstrap
puts the generated root password in the OS credential store and refuses a machine with none. The
`test` job installs `gnome-keyring` and runs the suites inside `dbus-run-session` from
`.github/scripts/test-no-network.sh`; the `bench` job does the smaller half of that — install, and
wrap this one command — rather than growing that script a second profile. The network namespace is
not reproduced: this suite talks to a `MockRegistry` on loopback like every other, and the isolation
the `test` job adds is a belt the `bench` job has never worn.

### D7 — The three archives are fetched by a script, not by three more copies of the step

The `test` job holds seven near-identical fetch steps, each about forty lines of `bash` inside YAML.
Adding three more copies to `bench` would make ten. `.github/scripts/fetch-package.sh` takes a kind,
a version and a destination, and does what those steps do: pick the target triple, pick the
extension, download, unpack with the right `tar`, and print the directory. The `test` job is left
alone — those steps carry per-package details (nginx packs a whole tree, Redis has no Windows-ARM
build, PHP has its own layout) that a shared script would have to grow flags for, and a refactor of
seven working steps is not what this task is.

## The interface

Nothing new on the API, nothing new in any crate that ships. This task adds a test, a CI script and
a CI step, and edits two documents.

## Crate changes

| Crate | Change |
| --- | --- |
| `mixengine-cli` (tests) | `tests/warm_start.rs` — the suite |

## Testing

`crates/mixengine-cli/tests/warm_start.rs`, `#[ignore]`d, one test:

`three_services_start_together_inside_the_budget`

1. Pack Caddy, MariaDB and Redis out of the three `MIXENGINE_*_PACKAGE` directories the CI step set;
   publish one signed index naming all three.
2. `package install` each, `service create` each on a free port, move Caddy's admin port off 2019.
3. Time `mix service start --json`. Assert three `running`, exactly one first-run job, and that it
   succeeded. **Report** the number as the first start.
4. `mix service stop --json`.
5. `WARMUP + RUNS` rounds of start → assert → stop, timing the start. Report each, report the
   median, and in a release build assert the median against ten seconds.

A watchdog thread of `mariadb.rs`' kind reports where the suite hung, because libtest holds a
running test's output until the test ends and this one starts nine servers.

## What was measured

Linux, in WSL2 Ubuntu 24.04 on this developer machine (12 cores, 8 GB), release profile, the
packages the CI steps pin — Caddy 2.11.4, MariaDB 11.4.12, Redis 8.10.0. Four runs of the suite:

| Run | First start (bootstrap) | Warm median | Warm rounds |
| --- | --- | --- | --- |
| 1 | 7416 ms | **1680 ms** | 13913, 3067, 1434, 1680, 1676 |
| 2 | 5185 ms | **3997 ms** | 3997, 2956, 6582, 12503, 1943 |
| 3 | 6605 ms | **3069 ms** | 3969, 3040, 3069, 7845, 2222 |
| 4 | 5497 ms | **4344 ms** | 2194, 5216, 4344, 5821, 4235 |

**The median passes with room, and the tail does not.** Every run is inside ten seconds on the
number the budget is about, and three rounds out of twenty were over it — 13.9 s, 12.5 s, and a 7.8 s
that was close. The gate is the median deliberately: one slow round on a machine also running a
browser is not a regression, and a gate on the maximum would flap. But a tail that reaches 14 s is a
user waiting 14 s, so it is written down here and in the roadmap rather than smoothed away.

**Where the time goes, from the daemon's own log**: a walk is sequential — `caddy` reaches `running`
in about 60 ms, `mariadb@main` takes 1.9–2.9 s, `redis@main` about 260 ms — so the number is a
*sum*, and it grows with the number of services rather than with the slowest. That is not a
discovery so much as a confirmation:
`crates/mixengine-daemon/src/services/mod.rs` says it in its own module note — *"A walk is sequential
over `Plan::flat` … the tiers are already computed, so M3's ten-second budget buys concurrency by
changing this walker and nothing else."* The budget did not need to buy it. A tier-parallel walker
is where the tail goes if somebody decides the tail matters.

Windows and macOS are the `bench` job's to report: this machine cannot run unsigned servers under
Smart App Control, and a number taken on one OS is not the milestone.

## Out of scope, and where each goes

- **A budget on the first start.** It is a different promise about a different run, and nobody has
  said what it should be. Reported here so that whoever wants to say it has a number.
- **nginx, MySQL, PostgreSQL, memcached.** The milestone names three services; measuring the other
  four is a second bench nothing has asked for.
- **A regression gate that trends over time.** CI keeps no history here, and a median against a
  fixed line is what T29 established.
