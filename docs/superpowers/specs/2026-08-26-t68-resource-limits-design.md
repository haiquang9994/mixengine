# T68 — `ResourceLimits` per OS

**Roadmap task:** T68, the first of phase 7.
**Status:** design, agreed 2026-08-26.

## What this is for

Phase 7's goal is that idle costs nothing. Three mechanisms carry it —
[`resource-isolation.md`](../../../.claude/features/resource-isolation.md) names them: on-demand
start, idle shutdown, and hard limits. T68 is the third, and it is the only one of the three that a
person sets deliberately rather than one that happens to them.

**The vocabulary already exists.** `ResourceLimits`, `Priority`, `IdleProbe` and `IdlePolicy` were
written into `mixengine-proto` when `ServiceSpec` was, and their doc comments name T68 and T69 as the
tasks that would give them effect. `services.limits_json` has been a column since the initial
migration, `generate.rs` has been parsing it into a `ServiceSpec` since T30, and every service in
every home has been carrying a `ResourceLimits::default()` through the whole of phases 3, 4 and 5.

So this task adds no type to describe a limit. What it adds is the two things that were deferred:

1. **Enforcement.** `spawn_supervised` reads a spec's `program`, `args`, `cwd` and `env`, and has
   never read its `limits`.
2. **Honesty.** The feature doc's macOS rule — *"no client may offer a memory-limit control that does
   nothing there, which means the API reports what the platform actually supports rather than a
   uniform shape"* — needs something to report *with*. There is nothing today.

And, because a capability nothing can reach is not shipped, a third: `service.limits` and
`service.set_limits`, with `mix service limits` over them.

## D1 — The read goes on `Host`, the write goes in `process`

This is `PortAccess`'s split, taken for `PortAccess`'s reason. What a machine *supports* is a
question, asked by the daemon, answered per OS, and mocked in tests — so it is a capability on
`Host`, beside `port_access` and `resolver`. Applying a limit is not a question: it happens to a
particular child, through the handle that spawned it, at the moment it is spawned or while it runs.
That belongs in `mixengine_platform::process`, where `Supervised` already lives.

The two halves never meet in the daemon's code. The daemon reads support to *report*, and passes
limits to *spawn*; it never consults one before doing the other, because a spawn that silently
dropped a limit the caller asked for would be exactly the dishonesty the feature doc forbids. What
happens instead is D6.

## D2 — Support is reported per field, and "unsupported" and "unavailable" are different answers

```rust
pub enum LimitMechanism { JobObject, CgroupV2, None }

pub enum WhenExceeded { AllocationFails, Killed }

pub enum Enforcement {
    /// A wall. `when` says what walking into it does.
    Hard { when: WhenExceeded },
    /// This operating system has no mechanism for this field. Nothing will make it appear.
    Unsupported,
    /// The mechanism exists here and this machine will not lend it. `why` is for a person.
    Unavailable { why: String },
}

pub enum MemoryMeasure { Commit, ChargedPages }

pub struct LimitSupport {
    pub mechanism: LimitMechanism,
    pub cpu: Enforcement,
    pub memory: Enforcement,
    pub memory_measure: MemoryMeasure,
    pub priority: bool,
    /// How many cores `cpu_percent` may be spent across, so a client can draw the ceiling it will
    /// otherwise be refused at. `cpu_percent: 800` is the whole of an 8-core machine.
    pub cores: u32,
}

pub trait ResourceControl {
    fn support(&self) -> LimitSupport;
}
```

**Per field rather than one flag for the pair, and Linux is why.** systemd delegates a user session's
cgroup subtree, but not every controller in it: `memory` and `pids` are delegated broadly, `cpu` is
not, and the difference moved across systemd releases. A machine where `memory.max` is writable and
`cpu.max` is not is an ordinary machine, not a broken one, and a single `available: bool` can only
describe it by lying about one of the two.

**`Unsupported` and `Unavailable` are different advice.** macOS has no hard memory cap and no release
will add one, so a client should not draw the control at all. A Linux machine whose session was
started without `cpu` delegation could have it — the answer to that person is a sentence about their
system, not a missing control. Collapsing them would produce the same silence for a permanent fact
and a fixable one.

**`memory_measure` exists because one number means two things.** A Job Object's
`JOB_OBJECT_LIMIT_JOB_MEMORY` bounds *commit charge*; cgroup v2's `memory.max` bounds *charged
pages*, which includes page cache. `ResourceLimits::memory_mb` says "resident memory" and cannot be
right on both. Rather than pick one and be wrong on the other system, the field is reported alongside
the number, and the CLI prints it. This is the `PortBinding { answer, bind }` idea again: the caller
is told what the number means here rather than left to assume it means the same everywhere.

## D3 — Linux: a cgroup discovered, not assumed, and a child that steps into it itself

Four steps, and each one can fail into `Enforcement::Unavailable` rather than into an error:

1. **Find the delegation boundary.** Read `/proc/self/cgroup` — under cgroup v2 that is one line,
   `0::<path>` — and walk up from the daemon's own cgroup to the highest ancestor in which this
   process can **create a directory**, which is the one capability the rest of this needs and is
   therefore the one that is tested for rather than inferred from ownership or from a path shape.
   Discovered rather than assumed: `user@N.service` is systemd's answer and this
   code may not be written against systemd's answer, because a machine with no systemd at all has to
   arrive here and be told so rather than have a path built for it fail to open.
2. **Enable the controllers.** `cpu` and `memory` must appear in `cgroup.subtree_control` at every
   level between the boundary and the service's own cgroup. Whichever of the two cannot be enabled
   becomes `Unavailable` for that field alone, and the other one still works.
3. **Create one cgroup per service**, at `<boundary>/mixengine/<service-id>/`. Plain names, with no
   `.slice` or `.scope` suffix: those suffixes are systemd's vocabulary, and a delegated subtree that
   uses them invites the thing that delegated it to start managing them back.
4. **Write the caps**, then hold `cgroup.procs` open.

**The child puts itself in, and that is the whole reason this is safe.** `unix/process.rs` already
registers a `pre_exec` closure — it calls `setsid`, and its documentation is a paragraph about
async-signal-safety. The cgroup join goes in the same closure, as `write(fd, "0\n", 2)` to the
descriptor opened at step 4. The kernel reads `0` in `cgroup.procs` as "the process doing the
writing", so there is no pid to format, nothing to allocate, and no call that is not
async-signal-safe.

The alternative — spawn, then have the daemon write the child's pid in — has a window between the two
in which the child may already have forked. For php-fpm, whose first act is to fork a pool, that
window is not theoretical: the workers would land outside the cap while the master sat inside it,
and the service would look capped while being uncapped. `pre_exec` closes it by construction,
because a process that joins before `exec` cannot have children yet.

**`memory.high` is set equal to `memory.max`, not below it.** Both are written, as the feature doc
asks. `memory.high` makes the kernel reclaim and throttle at the threshold; `memory.max` makes it
kill there. Setting `high` to the same value means a service that can be squeezed back under the line
is squeezed rather than killed, and one that cannot is still killed — which is what a development
machine wants, and needs no ratio anybody would have to defend. `WhenExceeded::Killed` remains the
honest summary: reclaim is what the kernel tries, being killed is what a person eventually sees.

**A cgroup outlives the process that was in it.** It is removed when the `Group` drops, and a daemon
that was killed leaves directories behind — so boot sweeps `<boundary>/mixengine/` for empty
cgroups, alongside the stale socket and pidfile cleanup T18 already does there. An empty cgroup is
removable and a non-empty one is a live service this daemon is about to adopt, so the sweep needs no
list of what it expects to find.

## D4 — Windows: the job object that is already there

`windows/process.rs`'s `Group` holds a job handle today, created before the spawn and carrying
`KILL_ON_JOB_CLOSE`. Limits are two more calls on that same handle, and no second object:

- `JobObjectCpuRateControlInformation` with `ENABLE | HARD_CAP`.
- `JobObjectExtendedLimitInformation` with `JOB_OBJECT_LIMIT_JOB_MEMORY`, and
  `JOB_OBJECT_LIMIT_PRIORITY_CLASS` for `Priority::Background`.

Set **before** the spawn, on a job that has no processes in it yet, so a process is capped from the
instant it is assigned rather than a moment afterwards. This is the same ordering the job's kill-on
close already relies on, and it costs nothing to keep.

**`cpu_percent` is divided by the core count here and nowhere else.** `ResourceLimits::cpu_percent`
is defined as a percentage of *one core*; `CpuRate` is expressed in hundredths of a percent of *all
cores together*. So `cpu_percent: 50` on an 8-core machine is `CpuRate = 625`. Linux needs no such
conversion, because `cpu.max`'s `$MAX $PERIOD` pair is already per-core by construction —
`50000 100000` is half of one core whatever the machine has. The asymmetry lives inside
`windows/process.rs` and nothing above it learns of it.

## D5 — macOS: priority, and a straight answer about the rest

`setpriority` in the same `pre_exec` as the cgroup join is on Linux, and `LimitSupport` answers
`mechanism: None`, `cpu: Unsupported`, `memory: Unsupported`, `priority: true`.

**The watchdog the feature doc describes is not built here.** *"warn, then optional restart at
threshold"* needs a per-process RSS sample taken repeatedly, and that sampler is T71 — 1 s sampling
while subscribed, with 24 hours of downsampled history. Building a second sampler inside T68 to serve
one field on one operating system would put a loop into the supervisor that T71 would then replace.
**Follow-up: a new task after T71**, added to phase 7 in the same commit as this spec so that the
deferral is a scheduled thing and not a note in a design document.

## D6 — A machine that will not lend the mechanism still starts the service

A service whose `memory_mb` cannot be enforced starts anyway, uncapped, and the daemon says so:

- `service.limits` reports the field as `Unavailable { why }`, every time it is asked.
- `mix doctor` grows one **`Note`**, not a `Problem`.

`Note` is T47a's distinction and this is what it was built for. A machine without `cpu` delegation is
not a broken machine, and reporting it as a fault would report the operating system as broken — the
same reasoning that keeps `hosts_only` a supported mode. It also means T47b's repair dispatch, whose
`match` has no wildcard arm on purpose, gains no arm: there is no repair, because there is nothing
here MixEngine may fix without asking a person to change how their session is started.

**The alternative was refusing the start**, and it was rejected because it converts a machine that
merely cannot cap a service into a machine that cannot run one. A blueprint carrying `memory_mb`
would then be undeployable on a stock non-systemd install, which is a worse product than an uncapped
service and a sentence explaining why.

## D7 — A limit applies immediately

`service.set_limits` writes `limits_json` and, if the service is running, writes the caps into its
job object or its cgroup before it answers. Both mechanisms accept a rewrite while processes are
inside them, so there is no state where a limit is set and not in effect, and therefore no
`pending, needs restart` flag for every reader of limits to carry and every client to render.

The cost is stated rather than hidden: **a service already over a newly-lowered memory limit can be
killed by the call that sets it.** On Linux the kernel reclaims first and kills if that fails; on
Windows the next allocation fails. That is the correct behaviour for the thing being asked for, and
`mix service limits set` prints the running state after the write so the person sees it happened.

`Supervised::set_limits` is the entry point, and it is the same code path the spawn uses, so a limit
applied to a running service and a limit applied at start cannot drift apart.

## D8 — The whole value, never a delta

`service.set_limits` takes a complete `ResourceLimits`. It does not take a patch.

This is T41's rule about the hosts block, applied to a much smaller thing for the same reason: a
delta needs a three-way value per field (`keep`, `set`, `clear`), which puts an enum into every
reader of limits including the ones that only want to display them. The whole value needs none.

**The cost is on the CLI, and it is paid by printing.** `mix service limits set web --cpu 50` sets
CPU to 50 and leaves memory uncapped and priority normal, because those are what the unnamed fields
are. It does **not** read the current value and merge — that is business logic in a client, which
CLAUDE.md forbids and which T46 refused by name when it added `domain.add` and `domain.remove`
rather than let a client compose one. What it does instead is print all three fields of the result,
so a memory limit that has just been cleared is on the screen.

## D9 — The platform speaks its own limits type

`process::spawn_supervised` takes a `mixengine_platform::process::Limits`, not
`mixengine_proto::ResourceLimits`, and the supervisor converts between them.

This is a layering constraint rather than a preference. The `process` feature does not enable
`dep:mixengine-proto` — only `host` and `elevated` do — and `mixengine-shim` compiles this crate with
`process` and without `host`. Reaching for the proto type here would add proto to the shim's
dependency closure to describe a value the shim never has.

## D10 — Refusals, written where they are written down

Two of the three are checked where the fact they depend on lives, and the split is not cosmetic —
one of these rules is about the value and the other is about the machine:

- **`memory_mb: Some(0)`** is refused by `ResourceLimits::validate` in `mixengine-proto`, beside the
  type, because it is wrong on every machine there will ever be. Zero is not "uncapped" — `None` is —
  and a service permitted to allocate nothing is a service that cannot start.
- **`cpu_percent` above `100 × cores`** is refused by the daemon's method handler, **not** by proto.
  Proto does not know how many cores this machine has and must not learn: it is the shared vocabulary
  and has no `Host`. The rule is real — the value converts, on Windows, into a share larger than the
  machine, and a ceiling above the ceiling is not a ceiling — but the number it is measured against
  is a property of the machine, which is where a rule about it has to be read.
- **`memory_mb` on macOS is not refused.** `LimitSupport` has already said it is `Unsupported`; a
  refusal here would mean a blueprint written for three systems fails to apply on one of them. It is
  stored, it is not enforced, and `service.limits` says so every time it is read.

## The API and CLI surface

| Method | Shape |
| --- | --- |
| `service.limits` | `ServiceTarget` → `{ limits: ResourceLimits, support: LimitSupport }` |
| `service.set_limits` | `ServiceTarget` + `ResourceLimits` → the same report |

Read and support come back together because neither is worth reading alone: `512` means nothing until
`memory_measure` and `Enforcement` are beside it.

```
mix service limits <service>
mix service limits set <service> --cpu 50 --memory 512 --priority background
mix service limits clear <service>
```

`clear` exists so that removing every limit is a named operation rather than a `set` with three
absent flags that a person has to infer the meaning of.

`service.create` is **not** changed. A recipe-declared default limit is a reasonable thing to want and
is T73's — the dev-tuned defaults pass over the service templates — not this task's.

## How this is proved

**Memory is proved by outcome; CPU is proved by reading back what was written.** These are not the
same strength of claim and the difference is deliberate.

Walking into a memory cap is a discrete event: the process dies, or an allocation fails. So
`mixengine-testkit`'s `fakeservice` — the debug-only recipe that already exists for exactly this kind
of fixture — gains a mode that allocates in steps, and the test sets `memory_mb`, starts it, and
asserts it is gone before it reaches twice the cap. `WhenExceeded` tells the test which of the two
endings to expect on the system it is running on.

A CPU cap is a rate, and asserting a rate means timing a busy loop on a shared CI runner. The warm
start suite already taught this project what that costs. So the CPU test reads the value back out of
the mechanism it was written into — `cpu.max` on Linux, `QueryInformationJobObject` on Windows — and
proves the cap was applied to the right object with the right number. **T68 does not prove that a CPU
cap slows anything down.** That measurement belongs to T72, which has a `bench` job that knows how to
compare against master.

| Layer | Suite | What it proves |
| --- | --- | --- |
| platform | `mixengine-platform` | the cgroup exists, the child's pid is in its `cgroup.procs`, the job object carries the flags and values written to it |
| daemon | `mixengine-daemon/tests` | `set_limits` reaches a running service; `mock::Host` returns `Unavailable` so the degraded path runs on all three systems without needing a machine that lacks delegation |
| CLI | `mixengine-cli/tests` | the three commands over a real socket, and that `set --cpu` prints all three resulting fields |

**Two machine facts to write down before somebody rediscovers them.** WSL without `systemd=true` has
no delegated subtree, so the Linux success path cannot be proved there and CI has to answer for it —
but that same WSL is the best place on this machine to exercise the `Unavailable` path. And
`linux/` and `macos/` are not compiled by clippy on Windows, so a per-OS file in this task is checked
in WSL before it is pushed.

## What this task deliberately does not do

- **No idle detection and no idle shutdown.** `IdlePolicy` stays unread; that is T69.
- **No activation gateway.** T70.
- **No sampling and no history**, and therefore no macOS memory watchdog. T71, and the follow-up task
  this spec adds after it.
- **No CI budget.** The idle-footprint and cold-path numbers, and the build failing on a regression,
  are T72.
- **No tuned template defaults.** T73.
- **No `pids` or I/O limits.** cgroup v2 offers both and Job Objects offer process counts; neither is
  in `ResourceLimits`, and adding a field to a shipped type to use a mechanism nobody asked for is
  how a limits API becomes a cgroup wrapper.
