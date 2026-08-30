# T73 — Dev-tuned defaults across the service templates

Roadmap: [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md).
Feature: [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md),
"Hard limits". Standard:
[.claude/standards/testing.md](../../../.claude/standards/testing.md), "Performance guards".
Predecessors: [T68](2026-08-26-t68-resource-limits-design.md), which named this task as the owner of
recipe-declared defaults; [T72](2026-08-30-t72-ci-budgets-design.md) and
[T72a](2026-08-30-t72a-cold-path-design.md), which both wrote *"no tuning — that is T73's"* and left
the `bench` job this one measures in.

## What this is for

`.claude/features/resource-isolation.md` says, in the middle of a section about cgroups:

> Defaults are conservative — MariaDB's `innodb_buffer_pool_size` and PHP's `memory_limit` are tuned
> down for a dev machine in our config templates, which saves more RAM than any cgroup will.

Half of that is true. PHP's ini set *is* tuned — T28 writes `memory_limit = 512M`, an opcache that
revalidates every request, and `display_errors = On`, none of which are PHP's own defaults. **The
database half is not.** Every number the database templates render today is the value the server
would have used with no configuration file at all:

| Recipe | Setting | Today | Whose number it is |
| --- | --- | --- | --- |
| mariadb | `innodb_buffer_pool_size` | `128M` | MariaDB's own default |
| mariadb | `max_connections` | `151` | MariaDB's own default |
| mysql | `innodb_buffer_pool_size` | `128M` | MySQL's own default |
| mysql | `max_connections` | `151` | MySQL's own default |
| postgres | `shared_buffers` | `128MB` | PostgreSQL's own default |
| postgres | `max_connections` | `100` | PostgreSQL's own default |
| memcached | `memory_mb` | `64` | memcached's own default |

So this task is a sentence in the feature document being made true, and the last open item in phase 7.

**It is tuning, not mechanism.** Nothing here adds an API method, a CLI command, a trait method or a
column. What lands is: a handful of directives in four templates, one new suite in the `bench` job,
and two documents corrected.

## D1 — Only the knobs that hold memory while nobody is looking

The phase this task closes is about what a machine costs when it is idle, so the rule for whether a
knob is worth turning is: **does it hold memory that a machine with no traffic is still paying for?**

Three groups pass that test.

- **Buffers allocated at startup.** `innodb_buffer_pool_size`, `key_buffer_size`, `shared_buffers`.
  These are present from the moment the server is ready and stay present with nobody connected.
- **Instrumentation nobody reads on a laptop.** MySQL's `performance_schema` is on by default and
  costs hundreds of megabytes of preallocated tables; MariaDB ships it off, and the template says so
  in one line rather than leaving the reader to know it. Nothing in MixEngine consumes either.
- **Durability of the log**, which is not memory but is the other half of what "tuned for a dev
  machine" has always meant, and is the change a developer feels most: a seed or a migration is
  thousands of small transactions, each one currently waiting for a disk flush.

**Two knobs fail the test and are deliberately left alone**, and this is the part of the task where
the roadmap line would have led somewhere worse:

- **`max_connections` is not lowered.** Per-connection memory is allocated per *actual* connection,
  so lowering the ceiling saves nothing at all on an idle machine. What it buys instead is a new way
  for a busy afternoon to fail — and that failure arrives as an error from MixEngine, in an
  application whose author did nothing wrong.
- **php-fpm's `pm.max_children` is not lowered.** T70 and T72a stop an idle pool outright, so the
  idle cost of a pool is already zero and there is nothing left for a smaller pool to save. Shrinking
  it would only make the machine slower while somebody is using it, which is the one time this phase
  never asked anything to be cheaper.

Redis, memcached, nginx and Caddy get nothing, and the templates already say why: Redis writes
nothing to disk and carries a `maxmemory` with a policy (T35); memcached's 64 MB is the number
`.claude/features/services.md` publishes; nginx runs one worker with `access_log off` (T43); Caddy
runs one process. **They were examined and left, and the spec says so** so that the next person does
not reopen four files to discover the same thing.

## D2 — Durability is relaxed on the log and never on the data

`innodb_flush_log_at_trx_commit = 2` and `synchronous_commit = off` both mean the same thing in two
dialects: a committed transaction is handed to the operating system rather than pushed through to the
platter. A power cut can therefore lose the last transactions — about a second's worth — and **cannot
corrupt the database**, because every write that establishes consistency still goes through its own
barrier.

What is **not** touched, and the distinction is the whole of this decision:

- `fsync` stays on in PostgreSQL. Turning it off risks an unrecoverable cluster, which is a different
  category of loss from "the last second is gone".
- `innodb_doublewrite` stays on for the same reason: it is what makes a torn page recoverable.
- Nothing is done to MariaDB's or MySQL's binary log, which is off in these builds anyway.

A developer whose laptop loses power mid-`migrate` re-runs the migration. A developer whose data
directory will not open has lost the afternoon, and MixEngine would have been the cause.

## D3 — The new directives are hard-coded in the template, not new settings

A [`Setting`](../../../crates/mixengine-core/src/generate/settings.rs) is a knob offered to the user.
These directives are not a knob; they are the sentence *this is a development machine* written in
each server's own dialect. That is exactly the shape Redis's template already uses for `save ""`,
`appendonly no` and `daemonize no` — stated, with the paragraph that says why, and no override key.

The escape hatch already exists and needs no new surface: `{{ extra }}` renders at the end of every
one of these files, and in all three formats — MySQL/MariaDB option files, `postgresql.conf`, Redis's
config — **a later line wins over an earlier one**. Anybody who wants production durability on their
own machine writes one line into `extra` and gets it.

The two size knobs that already *are* settings — `innodb_buffer_pool_size`, `shared_buffers` — stay
settings and only change their default. Nobody's override stops working.

## D4 — Every directive must exist in the oldest version the index publishes

This is T72a's lesson applied before it can cost anything. `pm.status_listen` was refused there
because php-fpm rejects an entire file over one unknown directive and this product offers PHP from
7.0; **MySQL's and MariaDB's option parsers behave the same way**, and
`.claude/features/services.md` offers MySQL from **5.6**, and the index publishes MariaDB from
**10.6**.

So the rule, and it is a rule rather than a checklist because the next person adding a line needs the
rule:

- A directive goes in unprefixed only if it exists in the oldest version the index publishes.
  `performance_schema` (5.5+), `key_buffer_size` and `innodb_flush_log_at_trx_commit` all do.
- Anything younger takes the `loose-` prefix, which downgrades an unknown option from a refusal to a
  warning — the spelling `loose-mysqlx = OFF` already uses in the MySQL template, for this reason.
- A directive that is *renamed* between versions — `innodb_redo_log_capacity` for
  `innodb_log_file_size`, 8.0.30 and later — is not worth having under either name, and is not added.

**Nothing here is trusted from documentation.** The first measuring round reads the values back out
of the running servers (`SHOW VARIABLES`, `SHOW server_version`) and prints them, so the "whose
number it is" column above is confirmed against the packages this project actually ships rather than
against a manual for some other build.

## D5 — The measurement is a difference, not an absolute

The `bench` job gains one suite: `crates/mixengine-cli/tests/tuned_footprint.rs`.

**It measures two MariaDB instances in one home.** `mariadb@main` takes the new defaults;
`mariadb@stock` is created with `config_overrides_json` — and an `extra` block — that puts every
value this task changed back to the server's own. Each is started alone, left to settle, and read
through `mix metrics --json`, which is T71's sampler and therefore the same number a person reads on
their own machine.

**What is gated is `stock − tuned`**, and that choice is the point of the whole suite:

- An absolute budget on MariaDB's RSS is a promise held hostage to next month's MariaDB, on a
  quantity this project does not control — which is exactly the reasoning `idle_footprint.rs` used to
  report the 60 MB total and gate only the daemon.
- A difference is the sentence `resource-isolation.md` actually makes, it is the only sentence a
  commit in this repository is responsible for, and it is far less sensitive to which runner it lands
  on: both readings are taken on the same machine, minutes apart, from the same binary.
- It also fails honestly in the direction that matters. **If the tuning does nothing, the suite goes
  red** — and "does nothing" is a real possibility worth guarding, because RSS is memory that has
  been *touched*, not memory that has been *asked for*. A smaller buffer pool that the allocator was
  never going to fault in would move no number at all, and a suite that gated an absolute would have
  passed while proving nothing.
- And it is expressed as a **fraction of the stock reading** rather than in megabytes, so that the
  gate says something about the configuration rather than about the runner — see D6.

The suite asserts its subject set before it compares anything, on `idle_footprint.rs`'s rule: an
instance that failed to start reports a wonderful number.

**Both readings are printed, every run.** The day the difference shrinks, the two numbers beside it
say whether the tuned side grew or the stock side shrank.

## D6 — The order of work: measure the method, then gate a fraction of it

1. The suite landed first, with both instances **rendering identical configuration**. That run
   (33322945686) is the baseline, and it measured two things: MariaDB's real RSS — 98.9 MB on
   Windows, 133.2 MB on Linux, 98.5 MB on macOS — and the method's own noise, at 0.0 %, 0.4 % and
   0.0 % between two servers given the same file.
2. The directives land second, and the same suite prints what they save.

**The threshold is a fraction, not a pinned number of megabytes**, and step 1 is why. Three machines
gave three different absolute numbers and next month's runner is a fourth, so a megabyte budget
would be a budget about this quarter's hardware. `SAVED_AT_LEAST = 1 %` sits two and a half times
above the measured noise floor and far below what the tuning is expected to give: it catches the
failure that matters — tuning that does nothing — without going red on a slow runner, which is how a
guard stops being read.

The measured saving in megabytes is reported by every run and written into
`.claude/features/resource-isolation.md`, which is where a number belongs when it describes a
machine rather than a rule.

## D7 — Two documents are part of the deliverable

- **`.claude/features/resource-isolation.md`** — the sentence quoted at the top becomes what the
  templates do, with the measured saving in it. A feature document that promises tuning nobody did is
  worse than one that promises nothing.
- **`.claude/roadmap/phase-7-efficiency.md`** — T73 is ticked with the number, and phase 7's `Done`
  column and the `Where we are` paragraph in `todo.md` follow it.

## What this task deliberately does not do

- **No recipe-declared `ResourceLimits`.** T68 named this task as the owner of "a recipe-declared
  default limit", and it is not built here: a default ceiling is a *mechanism* — a service that dies
  or is restarted for a reason its owner did not set — and this task is a tuning pass. A template
  value that makes a server ask for less memory and a job object that kills it for using too much are
  not the same promise. It stays T68's open item and is written down as such.
- **No `mix service set`.** `postgres.rs`'s module note already records that there is none; the stock
  instance in D5 is created with its overrides rather than changed after the fact. Adding the command
  to serve a test is how a test surface becomes a product surface.
- **No numbers for MySQL and PostgreSQL.** The `bench` job fetches Caddy, MariaDB, Redis and three
  PHPs; adding two more servers to it costs the archives, the bootstraps and a second credential-store
  dance on Linux, for a second copy of a difference MariaDB already demonstrates. What the `test` job
  proves for all four is the thing that would actually break — that a real server of that product
  accepts the file — since `mysql.rs`, `postgres.rs`, `mariadb.rs` and `redis.rs` each start one.
- **No change to the PHP ini set.** T28 tuned it and it is already what this document's opening quote
  claims.
- **No per-machine sizing.** Reading total system RAM and scaling the buffer pool to it is a
  reasonable product feature and a bad thing to hide inside a tuning pass: it would make two machines
  render different configuration from the same state, which is a change to what "generated config is
  disposable" means. If it is wanted, it is its own task.

## Testing

| Layer | Suite | What it proves |
| --- | --- | --- |
| unit | `mixengine-core`, per recipe | the rendered file carries each directive with the value the recipe declares, and an `extra` override still lands after it |
| real server | `mixengine-cli/tests/{mariadb,mysql,postgres,redis}.rs`, `test` job | the server this product ships **accepts the file** — the failure D4 exists to prevent is a refusal to start, and it is caught on all three systems |
| bench | `mixengine-cli/tests/tuned_footprint.rs` | the tuning saves memory, as a difference, in release, on all three systems |

**The mutation check for the bench suite** is the one D5 already describes: with the directives
reverted, the difference is zero and the gate goes red. That is run once by hand before the threshold
is pinned, the same way T72a pointed its status path at `/index.php` to watch its own assertion fail.

**What the real-server suites are not asked to prove** is that a relaxed flush is faster. Timing a
seed on a shared runner is the measurement this project has already learned not to make — the warm
start's bimodality on ubuntu is the standing lesson — and the saving in D2 is claimed as a smaller
number of `fsync` calls, which is a fact about the directive rather than a number about a machine.
