# T72a — A pool on a socket that can be idle-stopped, and a budget on the first request

Roadmap: [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md).
Feature: [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md),
"Cold path". Standard:
[.claude/standards/testing.md](../../../.claude/standards/testing.md), "Performance guards".
Predecessors: [T69](2026-08-26-t69-idle-detection-design.md), whose sweeper and whose unused
`HttpCounter` this finishes; [T70](2026-08-29-t70-on-demand-activation-design.md), whose activator
the first request walks through; [T72](2026-08-30-t72-ci-budgets-design.md), which split this out and
whose D4 and D5 are built here.

## What this is for

One published number is still not measured anywhere: **the first request to a stopped site is served
in under 1.5 s**. T72 gated the other one and left this, on the finding that on two systems of three
there is no *stopped site* for a first request to arrive at. This task makes there be one, and then
gates the number on all three.

Milestone **M7**'s second half is claimed when this lands, and not before.

## What T72 got wrong, and what is actually missing

**T72's diagnosis names three things and only one of them is true.** It says that on Linux and macOS
`activation_port_needed`, `activator` and `held_while_stopped` all answer nothing, so a pool is never
idle-stopped and nothing would wake it. Read against the code:

- `activation_port_needed` answering `false` on a socket is **correct and deliberate** — there is no
  number to allocate, which is exactly what the trait's own documentation says it means.
- `activator` **does** answer for a socket pool, and has since T70:
  `recipes.rs`'s `activator_socket` derives `php-fpm-8.3.activate.sock` beside the pool's own socket,
  and `php_fpm.rs`'s own test asserts it in as many words — *"a socket pool derives its activator and
  needs no row; a TCP pool cannot"*. The daemon binds it at boot through `activate::hold_all`, and
  both site templates already render it as the second upstream under `lb_policy first`.
- `held_while_stopped` being empty for php-fpm is **correct and deliberate**, and its own
  documentation says why: a pool has a front end in front of it, so it gets a permanent activator
  instead of having its own address held.

So the roadmap's instruction — *hold the socket while the pool is stopped, render it as the site's
second upstream* — describes work that was finished by T70 and T70a. **What is actually missing is one
line**: `php_fpm.rs`'s `idle_probe` is `context.port().map(...)`, and a pool on a socket has no port.
No probe means `generate.rs` attaches no `IdlePolicy` — *"a recipe that declares no probe is never
idle-stopped however its row is set"* — so the pool runs forever and there is nothing to wake.

**T69 recorded the same fact from the other side** (*"a php-fpm pool on a Unix socket is never
idle-stopped"*) and T72 inferred the wrong cause for it. This task fixes the cause, and corrects both
documents that carry the wrong one.

## D1 — The probe asks php-fpm, and asks it over the pool's own socket

`IdleProbe` counts TCP connections, and there is no cross-platform way to count connections to a Unix
socket: Linux publishes them in `/proc/net/unix` — measured, one row per connection with the path on
it — but macOS's `lsof` has no state filter for a Unix socket the way `-sTCP:ESTABLISHED` is one for a
port, and the honest alternative there is `libproc` through FFI. **So the question is not put to the
operating system at all. It is put to php-fpm**, which has kept the answer since PHP 5: the pool
config gains

    pm.status_path = /mixengine-status

and the daemon asks for it by speaking FastCGI to the pool's own socket. **No per-OS code, on any of
the three systems** — which is why this task loses the roadmap's **(P)** marker.

**`pm.status_listen` is deliberately not used, and that decision is this design's sharpest.** A status
listener of its own would be strictly better arithmetic — measured, a probe over it leaves `accepted
conn` untouched and reads `active processes` as 0 — but it exists only **from PHP 8.0**
([php-src `PHP-8.0/UPGRADING`](https://raw.githubusercontent.com/php/php-src/PHP-8.0/UPGRADING)), and
an unknown directive is not ignored by php-fpm: `php-fpm --test` refuses the file, so the pool would
not start. MixEngine offers **PHP from 7.0 upwards on purpose** — `mixengine-packages`' `eol.json`
argues it at length: *"the people who reach for a local development environment rather than a
container are very often the people maintaining something old"*. A design that idle-stops 8.x and
leaves 7.4 running forever abandons exactly the person that sentence is about, and a design that
branches on the version has two definitions of *idle* in one feature.

The case `pm.status_listen` was added for — its own changelog says *"useful for getting status when
all children are busy with serving long running requests"* — is a case this sweeper does not need
solved. When every worker is busy the probe queues and times out, which is `Unmeasurable`, which
means **the pool is not stopped**. That is the answer the correct arithmetic would have produced
anyway.

**The status path must not end in `.php`.** Both front ends hand FastCGI only what matches `.php` —
Caddy's `php_fastcgi` after its `try_files` rewrite, nginx's `location ~ \.php$` — so no URL from
outside can produce a `SCRIPT_NAME` equal to `/mixengine-status`. That is an argument, not a
measurement, so D5 turns it into a test rather than leaving it here.

**Every existing home restarts its pools once.** The pool file changes, `document::install` sees the
change, and the pool is restarted to pick it up. Said here rather than discovered by a user.

## D2 — What "idle" means, and why it takes two readings

Measured against php-fpm 8.3.6, a pool on a socket with `pm = static`, `pm.max_children = 5`:

| | idle | one 1.5 s request in flight | the sample after it, same request |
|---|---|---|---|
| `accepted conn` | previous **+ 1** | previous + 2 | previous + 1 |
| `active processes` | 1 | 2 | 2 |

A probe is itself a request, so it costs exactly one `accepted conn` and occupies exactly one worker
— stable across every reading taken. **Idle is therefore `accepted conn == previous + 1` and
`active processes <= 1`, and it needs both halves:**

- **`accepted conn` alone is blind to a long request.** Column three is the proof: a request that
  spans several samples increments the counter once, in the first minute, and the samples after it
  see the counter advance by the probe alone. A sweeper reading only that would stop a pool in the
  middle of serving.
- **`active processes` alone is blind to everything between samples.** At one reading a minute, a
  site under a steady stream of 50 ms requests is almost certainly between two of them at the moment
  it is asked, and would be stopped as unused.

**`start time` decides whether the two readings are comparable at all.** A pool that restarted has
reset its counter, so a `now` below `previous` is not a quiet minute — it is a different pool. The
sample is treated as activity: count from the start rather than stopping on a number that means
nothing.

**Anything that is not a clean reading is `Unmeasurable`, never idle.** A refused dial, a timeout, a
body that is not JSON, a JSON without both fields. That is `ConnectionCount`'s documented rule, which
this task does not weaken: *"reading I could not measure as there is nothing to measure stops a
service somebody is using"*.

**The first sample after a pool starts is never idle**, and this needs no new code: `observe`'s
existing arm already folds "no baseline" into "busy", with the reason written down — *"a service with
no baseline is one this build cannot call idle, and saying so as busy is the answer that keeps it
running"*.

## D3 — One variant, and less new code than the task looked like

`IdleProbe::HttpCounter` is **not unused**: T69 implemented it fully, including `Counters` (the
per-service memory of the last reading), the no-baseline rule above, and `counter_in`, whose own
documentation names the endpoint it was written for — *"php-fpm's `?json`"*. T69 anticipated this
road; what it could not do was reach php-fpm, which does not speak HTTP.

So the shape of the change is small:

- **`mixengine-proto`**: `IdleProbe::FastCgiStatus { socket: PathBuf, path: String }`. Not a
  general address: Windows has no use for it (below), so a socket is all it can carry, and a variant
  that cannot express a wrong thing is better than one that can.
- **`mixengine-supervisor`**: one more arm in `observe`, keeping the no-baseline rule unchanged. It
  differs from `HttpCounter`'s arm in exactly two ways — it reads three fields rather than one, and
  it compares `+ 1` rather than `==` — and both differences are D2's, not the protocol's.
- **`Counters` grows from a number into a reading**, and this is the one place the change is not
  additive. It is `BTreeMap<ServiceId, u64>` today, which cannot hold D2's second half: the sweeper
  has to remember `accepted conn` **and** the `start time` it was read against, or it cannot tell a
  quiet minute from a pool that restarted and reset its counter. So the value becomes a small enum —
  one variant per probe that remembers anything, `HttpCounter`'s carrying the number it carries
  today. A remembered reading of the *other* variant is treated as no baseline at all, which is the
  honest answer on the sweep after a service's spec changed shape.
- **`mixengine-core`**: `php_fpm.rs`'s `idle_probe` answers the new variant on the socket arm; the
  template gains one line; the status path is a constant beside `POOL_FILE`.
- **`mixengine-cli`**: one arm in `render.rs`'s `probe`, so `mix service show` can say what it is
  watching.

**Windows keeps `Connections { port }` and that is not a compromise.** It runs `php-cgi.exe -b addr`,
which is not php-fpm and publishes no status; and it already has a working probe, a real port and an
allocated activation port. The two systems measure *idle* by different mechanisms for the same
service, which is the same shape `LimitSupport` has: ask the platform what it can answer, rather than
insisting all three answer alike.

## D4 — Where the FastCGI client lives

`mixengine-supervisor/src/fastcgi.rs`, beside `http.rs` for the same reason `http.rs` is there: the
thing that needs it is `observe`. Async, one `BEGIN_REQUEST` / `PARAMS` / empty `STDIN`, records read
until `END_REQUEST`, under `idle.rs`'s existing `PATIENCE` (2 s) rather than a deadline of its own —
one sweep's patience is one number, and a probe that outlived it would delay the sweep it belongs to.
It
dials through `mixengine_platform::activation::dial`, which is the call the activator already makes,
so this crate gains no `#[cfg]`.

**There is already a FastCGI client in this workspace** — `mixengine-testkit`'s, 298 lines, blocking,
written for `php_fpm.rs`'s suite. It cannot be the one the daemon uses: testkit is a dev-dependency
and `workspace_layering.rs` enforces that. Two encoders of one protocol is a drift risk this design
accepts, with its eyes open and for a bounded reason: the record header has been eight fixed bytes
since 1996, both sides have tests, and the alternative — a `mixengine-testkit` → `mixengine-supervisor`
edge added so a pure encoder can be shared — buys less than a new edge in the layering costs. If the
implementation finds the sharing cheaper than this paragraph predicts, take it and amend this note.

## D5 — The status path is proved unreachable, not argued unreachable

An integration test through a real front end: `GET /mixengine-status` against a served site must
answer **404**, and must not answer php-fpm's JSON. D1's argument for why it cannot is the kind this
repository requires a measurement for — and the exposure is a real one now that the status page
shares the socket the sites use, where `pm.status_listen` would have made it structural.

## D6 — The cold path suite, and the contradiction it has to settle

`crates/mixengine-cli/tests/cold_path.rs`, in the shape T72's D4 already argued: **three pools, three
sites, one sweep**. A round is a single `GET` to a site whose pool is stopped, timed to the last byte,
asserting 200, asserting the body is what the PHP prints, and asserting the pool was `stopped` before
and `running` after. The pool must have been stopped **by the sweeper** — a service a person stopped
is one the activator refuses to wake, deliberately (T70 D8) — so the suite sets an idle policy and
waits for a sweep.

**T72's D4 and D5 do not agree, and this settles it.** D4 wants three pools; D5 adds one PHP to the
`bench` job's fetch step. Three pools means three `runtime_installs`, which means **three PHP
versions fetched**. The alternative — one PHP, three rounds, a fresh wait before each — costs about a
minute per round on every OS leg, and buys nothing.

The reason for three rounds is stronger than T72's *"the second and third are more realistic"*: **a
single CI measurement has already misled this project once** — the warm-start bench is bimodal on
ubuntu, and a red there has meant a bad minute rather than a regression. Three numbers admit a
median. That is the argument, and realism is the bonus.

Gated at **1.5 s**, release-only, printed in debug — `idle_footprint.rs`'s shape exactly. The step
runs **before M3**, on T72's finding that a failing step ends its job and the cheap independent
measurement should not be lost behind somebody else's flake.

## D7 — What this task does not do

- **No API and no CLI addition.** `service.set_idle`, `site.create` and `metrics.snapshot` are all
  T69's, T70's and T71's.
- **No probe for the databases.** They listen on a port on every system and their existing probe is
  correct; nothing here reaches them.
- **No `pm.status_listen`, on any version.** D1.
- **No tuning.** If 1.5 s is missed, this task writes down what it measured and raises it; making the
  wake faster is T73's.
- **No change to Windows.** Its cold path already worked and is already measurable.

## Testing

- **Unit, in `mixengine-supervisor`**: the two-reading rule against captured php-fpm bodies — a
  counter that advanced by one is idle, by two is busy, an `active processes` of 2 is busy whatever
  the counter did, a `start time` that moved is busy, a body missing either field is `Unmeasurable`.
  These are the four ways D2 can be got wrong.
- **Unit, in `mixengine-core`**: the socket arm renders `FastCgiStatus` and the Windows arm renders
  `Connections`; the rendered pool file carries the status path; the status path does not end in
  `.php` (an assertion on the constant, because D5's test cannot run everywhere).
- **Integration, `php_fpm.rs`**: against a real PHP, a pool answers its status page over FastCGI, and
  the counter advances by exactly one per probe. This is the measurement D2 rests on, made a test so
  that a php-fpm that changes its accounting is a red rather than a surprise.
- **Integration, front end**: D5's 404.
- **Bench, `cold_path.rs`**: D6.

## Documents to update

- [.claude/features/resource-isolation.md](../../../.claude/features/resource-isolation.md) — the
  "Cold path" bullet stops saying *"it is given no activator"*, which is wrong, and the criterion
  *"a request to an idle site succeeds within the cold-path budget"* stops being a promise.
- [.claude/roadmap/phase-7-efficiency.md](../../../.claude/roadmap/phase-7-efficiency.md) — T72's
  entry gains one correcting sentence rather than being rewritten (it is a record of what was
  believed), T72a is ticked with the measured numbers and loses its **(P)**.
- [.claude/standards/testing.md](../../../.claude/standards/testing.md) — the cold-path guard joins
  the other three in the same shape.
- [.claude/operations/build-and-release.md](../../../.claude/operations/build-and-release.md) — the
  `bench` job grows a step and two PHP fetches.

No ADR. The number was argued for long before this task, and asking a service about itself rather
than asking the operating system about it is within `mixengine-platform`'s rule rather than an
exception to it.

## Order of work

1. `fastcgi.rs` in the supervisor, with its own tests against captured records.
2. The `FastCgiStatus` variant, the `observe` arm, and D2's four unit tests.
3. `php_fpm.rs`: the probe arm, the template line, the constant — and the `php_fpm.rs` integration
   test that proves the counter advances by one.
4. D5's front-end test.
5. `cold_path.rs`, measured and printed, **not yet gated**; the `bench` job's three PHP fetches and
   its step.
6. **One CI run to read three numbers**, on three systems.
7. Turn the gate on at 1.5 s — or, if a system cannot meet it, write down what it measured and raise
   it as a product decision rather than editing the number.
8. The four documents, and the roadmap entry with the numbers in it.
