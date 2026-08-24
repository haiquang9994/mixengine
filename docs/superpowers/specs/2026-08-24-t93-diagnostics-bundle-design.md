# T93 — `daemon.bundle`, the diagnostics archive a client does not assemble

Roadmap task **T93**, phase 4, and the last of it that can be built from here. Design settled
2026-08-24, before implementation. It reads what [T47a](2026-08-24-t47a-doctor-design.md) built and
writes nothing that [T47b](2026-08-24-t47b-doctor-repair-design.md) would repair.

Everything under `.claude/` that this build touches is in force: no business logic in clients, no
client-only capability, no OS calls outside `mixengine-platform`, no persistent root process,
generated config is disposable, cross-platform or not merged.

## Scope

In: `daemon.bundle`, the five members it packs, where the archive lands, what it refuses to include
and how it says so, and `mix doctor --bundle`.

Out, each with an owner: the operating system's own version string is **not in this bundle** and D8
says what would put it there; `etc/` and the declared state tables are **omitted by name** and D4
says what adding them costs; the complete uninstall path is **T87**.

---

## D1 — The task's word "redacted" is already spent, and not here

T93 reads "credentials redacted". The obvious reading is a scrubber over the log — a pass that
looks for `password=` and replaces what follows.

**That is not what this build needs, because
[ADR 0006](../../../.claude/decisions/0006-servicespec-in-proto-and-secret-free.md) already won
it, at the type level, and named this bundle while doing so.** A `ServiceSpec` may *name* a
credential (`EnvValue::Keyring { service, key }`) and cannot carry one; `Step` and `SecretFile`
have hand-written `Debug` implementations so that a `tracing` field on a failed bootstrap prints a
length and never a value; the MariaDB recipe's own module doc says the root password lives in the
keyring and "never in the rendered file". ADR 0006's argument is explicit that moving a struct
between crates would leave the hazard "in the database, the logs and the diagnostics bundle" — the
remedy it chose instead is the one this task inherits.

A scrubber added on top would be strictly worse than nothing: it is a guess that a pattern matched,
and its presence invites the next contributor to believe the log is filtered rather than clean.

So **nothing in this bundle is redacted, because nothing that may hold a credential goes into it.**
What this task owes is not a filter. It is D2.

## D2 — The member list is a closed enum, and never a directory sweep

Five members, and the set is a Rust enum with an exhaustive `match` and no wildcard arm — the same
shape T47a chose for `ProblemId` and T47b spent on `plan_for`.

```rust
/// Everything this build puts in a bundle. Closed, so a file added to the home later cannot arrive
/// in an archive by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Part {
    /// What this bundle is, what is in it, and what was left out.
    Manifest,
    /// `daemon.doctor`'s answer, verbatim.
    Doctor,
    /// `daemon.status` and `daemon.version`, together.
    Status,
    /// The facts a reader needs to place the other four on a machine.
    Platform,
    /// The tail of `daemon.log`.
    DaemonLog,
}
```

**Why closed, concretely.** `<root>/certs/` will hold the internal CA's private key,
`<root>/data/` holds the user's databases, and `<root>/run/` is what `Paths::private_directories`
calls "what stands between a local process and the daemon". A sweep of the home written today omits
all three because whoever writes it remembers them. A sweep still omits them next year only if
every person who adds a file to the home happens to think about an archive somebody emails. A
closed enum removes the "happens to": a new member is a variant somebody wrote down, and a `match`
that stops compiling is what asks them to.

This is the same trade T47a made for a different reason and the argument transfers: keyed off a
spelling, a rule silently stops matching; keyed off a variant, it does not compile.

## D3 — What each member holds

| Member | File | Contents |
| --- | --- | --- |
| `Manifest` | `manifest.json` | Format number, the moment, the home, the daemon's version and protocol, the parts, the omissions, and the log accounting from D5. |
| `Doctor` | `doctor.json` | The `DoctorReport` from `Doctor::report()`, serialised as the API sends it. |
| `Status` | `status.json` | `DaemonStatus` and `DaemonVersion` — uptime, pid, endpoint, database, elevation summary, DNS mode. |
| `Platform` | `platform.json` | D4. |
| `DaemonLog` | `daemon.log` | D5. |

```rust
pub struct Manifest {
    /// The shape of this file. `1` today; a reader that does not know a number stops.
    pub format: u32,
    pub taken_at: Timestamp,
    pub home: String,
    pub daemon: DaemonVersion,
    /// Every part in this archive, including this one.
    pub parts: Vec<Part>,
    pub omitted: Vec<Omission>,
    pub daemon_log: LogExcerpt,
}
```

**The manifest names the parts and the answer sizes them.** `BundleReport::members` carries a byte
count per part and `Manifest::parts` does not, because the manifest is written last and cannot
state its own size from inside itself. Rather than one member carrying a hole, the sizes live where
every one of them is already known: in the value the call returns, after the archive is closed.

**`doctor.json` is the report and not a rendering of it.** A bundle carrying `mix doctor`'s
human-facing text would be a bundle whose contents change when the CLI's margins change, and would
lose the `ProblemId` a reader wants to grep for.

**`status.json` is the value `daemon.status` answers with, built by the same code.** The handler
reads it through `Api::status` and hands it in, rather than `Bundles` assembling a second one from
the same fields — two constructions of one document are two things to keep in step, and the one
inside the archive is the one nobody would notice had drifted.

**A part that could not be read becomes an omission carrying the error, and the call still
succeeds.** `Api::status` is fallible — the elevation queue can refuse a read — and a bundle that
failed outright because of it would be an archive lost at exactly the moment somebody needed one.
So the archive has four members and `omitted` names the fifth with the wire error as its reason.

This is not the same as D5's empty log, and the difference is the one
[`Keyring::secret`](../../../crates/mixengine-platform/src/traits/keyring.rs) already draws: a
daemon that has logged nothing is an *answer*, and a read that failed is a *failure*. An empty
member says "there was nothing"; an omission says "there was something and I could not get it".

## D4 — `platform.json` carries the facts, `doctor.json` carries the judgement

The two would otherwise overlap badly: T47a's checks already probe the resolver, port access and
the reserved ranges, and each renders its finding as a sentence.

The split is **judgement versus the facts it was made from**, and it has a second consequence that
decides the design: `platform.json` holds only what is **free to read**, so taking a bundle does
not probe this machine a second time. A resolver probed twice in one call could answer differently
each time, and a bundle whose two members disagree about the machine is worse than one that says
less.

Free, and therefore in:

- `std::env::consts::OS`, `ARCH` and `FAMILY` — compile-time constants, so this is not an OS call
  and does not belong behind a `mixengine-platform` trait.
- The daemon's build version and `ProtocolVersion`.
- `mixengine_platform::orphan_guarantee()` — a constant per OS, and
  [ADR 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md)'s subject.
- `ElevationSupport`, as the machine-readable value rather than as a sentence.
- The reserved port ranges, as numbers.

Not free, and therefore left to `doctor.json`: the resolver's state and the port-access probe, both
of which do I/O and both of which T47a already ran.

**Three things are omitted by name rather than silently**, and each appears in
`manifest.json`'s `omitted` with its reason:

- `etc/`, the rendered configuration. It is the most useful thing a configuration bug report could
  carry and it is an enumerable set — `Generator::declared()` returns every `Document` this home
  renders, so including it would not be a sweep. It is out for a different reason: it is the one
  surface a person edits by hand, which is precisely what T47b's `generated_config_stale` check
  exists to find, so it is the one surface ADR 0006's guarantee does not cover. Adding it later is
  one variant and one arm — and the decision to add it should be made on that sentence, not on
  convenience.
- `data/`, `certs/` and `run/` — the private directories, per D2.
- The declared state tables (`services`, `sites`, `projects`). Out for scope only: T93 names four
  things and these are not among them. They are secret-free by ADR 0006's contract, so this is the
  cheapest member to add when somebody wants it.

## D5 — The log excerpt states where it begins

`daemon.log` rotates at 10 MB × 5 (T4). The bundle carries **the last 1 MiB of the current file**
and no rotated file at all.

A cut at a byte offset lands mid-line, so the first partial line is dropped rather than shipped as
a fragment that reads like a malformed record.

The manifest carries what the cut cost:

```rust
pub struct LogExcerpt {
    /// Bytes of `daemon.log` in the archive.
    pub included_bytes: u64,
    /// Bytes of the current file that were older than the cut, including the dropped partial line.
    pub skipped_bytes: u64,
    /// Rotated files that exist beside it and are not here.
    pub rotated_files: u32,
}
```

**An excerpt silent about where it starts is an excerpt claiming to be the whole log.** The reader
of a bug report needs to know the difference between "nothing was logged before this" and "1.4 GB
was logged before this", and the difference is three numbers.

A home whose daemon has not written a log yet gets an empty member and an accounting of zeroes,
not an omission — the member list is closed, and a part that is present-but-empty is a fact.

## D6 — The daemon writes the file; a client is handed a path

```rust
/// What a caller asks for. Empty today, and a struct rather than no parameter so that the first
/// option to arrive is not an API change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsBundle {}

pub struct BundleReport {
    /// Where it was written, absolute.
    pub path: String,
    /// The finished archive's size on disk.
    pub bytes: u64,
    /// When it was taken.
    pub taken_at: Timestamp,
    /// Each part and the bytes it contributed, in packing order.
    pub members: Vec<Member>,
    /// What was deliberately left out, and why.
    pub omitted: Vec<Omission>,
}

pub struct Member { pub part: Part, pub bytes: u64 }
pub struct Omission { pub name: String, pub because: String }
```

**A path is a complete answer here and a stream would not be.** The transport is a Unix socket or a
Windows named pipe, so every client of this daemon is on this machine by construction; "copy
diagnostics" in a graphical client is opening a file whose path it was given. A second HTTP route
beside `/logs` would be a streaming surface for something that is not a stream, and a failure
halfway through it has no error shape to return — the JSON-RPC answer does.

**The daemon writes inside its own home and nowhere else.** `<root>/cache/diagnostics/`, named
`diagnostics-<YYYYMMDD>T<HHMMSS>.<mmm>Z.zip` from `Timestamp` — sortable, unique per millisecond,
and readable in an email. `cache/` and not `run/`: `Paths::cache` is documented as the disposable
directory that survives a reboot, and it is deliberately *not* private, which is the correct
permission for a file whose entire purpose is to be sent to somebody else.

The method takes **no destination**. `mix doctor --bundle --out FILE` copies, in the client. A
parameter naming a path would make `daemon.bundle` a way for any local caller to have the daemon
write a file anywhere that daemon can reach.

Three bundles are kept. On each call, anything older in `cache/diagnostics/` beyond the newest
three is removed — the reason to keep more than one is to compare two runs of a support
conversation, and that is what three is for. **A failure to prune does not fail the call**: the
bundle the caller asked for exists, and refusing to hand it over because an old file would not
delete would be the archive lost for the sake of the tidying.

## D7 — `.zip` on all three operating systems

Not `.tar.gz`, although `tar` and `flate2` are already dependencies and the publishing pipeline
packs `.tar.zst`.

The distinction is who opens it. A release artefact is opened by an installer this project wrote; a
diagnostics bundle is opened by **a person who was sent one**, on an operating system nobody chose
for them. Explorer has understood `.zip` since Windows XP and only learned `tar` in Windows 11
23H2. `zip` is already a workspace dependency with `deflate-flate2`, so this costs no new crate and
no second format.

One format on all three, not a format per OS: a bug report travels between machines, and the person
who receives it is rarely on the machine that produced it.

## D8 — The operating system's version is not in this bundle

It is the fact a bug report asks for first, and it is genuinely absent: `std::env::consts::OS`
gives `"windows"`, never `"Windows 11 26200"`.

Reading it is a system call, so
[the workspace rule](../../../.claude/CLAUDE.md) puts it behind a `mixengine-platform` trait, and
there is no existing capability it belongs to — `HomeDirs` is about paths, `DirectoryAccess` about
permissions. It is a new trait with three implementations and a mock, which is a task and not an
arm of this one. Calling `sysinfo` from the daemon instead would be the direct OS call outside
`mixengine-platform` that the workspace forbids, so there is no shortcut worth taking.

What would open it: a `SystemFacts` capability carrying the OS version, the kernel version, the CPU
count and the physical memory — all four of which the metrics work in
[client-surface.md](../../../.claude/features/client-surface.md) will want from `sysinfo` anyway.
It should be built when that is, and `platform.json` gains a field.

Recorded here rather than left as a gap for a reader to rediscover.

## D9 — `mix doctor --bundle`, and why it exits zero

```
mix doctor --bundle [--out FILE]
```

`--bundle` **conflicts with `--repair`**. A bundle taken after a repair describes a machine that no
longer has the problem it is being sent about; the two are separate intentions and combining them
in one invocation would produce the less useful of them silently.

**It exits zero when the archive was written**, unlike bare `mix doctor`, which exits non-zero when
it found a problem.

The deliverable of the command is the file. A bundle is taken *because* something is wrong, so
exiting non-zero every time would make the ordinary success look like a failure to the person
reading their terminal and to the script that wrapped it. What the exit code answers is "did I get
the archive", and the answer to "is this machine well" is inside the archive, where the person
asking it will be looking.

`--json` prints the `BundleReport`. The human rendering prints the path, the size, and one line per
omission — the omissions are the half a person will not otherwise know to ask about.

## D10 — Where the code lives

- `crates/mixengine-proto/src/bundle_api.rs` — `DiagnosticsBundle`, `BundleReport`, `Part`,
  `Member`, `Omission`, `Manifest`, `LogExcerpt`.
- `crates/mixengine-daemon/src/diagnostics.rs` — `Bundles`, holding the `Doctor` it reads, the
  `Paths` it writes under, and the readings `status.json` needs.
- `crates/mixengine-cli/src/main.rs` and `render.rs` — the flag and its rendering.

**Nothing in `mixengine-core`.** This is assembly across subsystems, which is where `doctor.rs` and
`repair.rs` already stand; core owns domain logic, and "which five files go in an archive" is not
one.

`Bundles` is built beside `Repairs` and holds the same `Arc<Doctor>`, for T47b's reason: the two
halves of one feature cannot be given different dependencies if they are handed the same object.

## D11 — Testing

**The assertion worth writing is the negative one, and it comes with a control.** A marker string
is written into `run/`, into `certs/` and into `data/`, and a *fourth* copy of the same marker into
`daemon.log`. The whole archive is then read as bytes: the first three must not appear anywhere in
it, and the fourth must. Without the fourth, three absences prove only that the search was looking
in the wrong place.

That is the test that actually asserts D2. It fails the moment somebody replaces the closed list
with a walk of the home, which no test on the member names would catch.

Beside it:

- The entry names in the archive are exactly `Part`'s five, no more and no fewer.
- The log excerpt begins at a line boundary, and `skipped_bytes` accounts for what a 1 MiB cut into
  a larger file dropped — asserted against a log written to be longer than the bound.
- A home with no `daemon.log` yet still produces five members, one of them empty.
- Pruning keeps three: four calls leave four files minus the oldest, and the three that remain are
  the three newest.
- `--bundle` with `--repair` is refused by clap, not at runtime.

**No test asserts that the machine running it is well**, which is T47a's finding and T34c's, twice
paid for: the archive's contents are asserted, the doctor report inside it is not.

## What this task does not settle

- The OS version string — D8.
- `etc/` in the bundle — D4 names the one sentence that decides it.
- The declared state tables — D4, scope only.
- Any bundle taken while the daemon is not running. `mix doctor` already requires one, and a client
  that has lost the socket has no daemon to ask for an archive. What a dead daemon leaves behind is
  `logs/daemon.log`, in its documented place.
