# T91 — Crash reporting (design)

Roadmap task **T91**, phase 9: *"Crash reporting that is opt-in and contains no project paths or
credentials."*

Three documents in this repository already describe a crash report that does not exist. `rust.md`
says the RPC layer turns a panic into `internal`; `api/rpc.rs` says the panic message *"has already
gone to the log through the panic hook"*; `Cargo.toml`'s release profile keeps symbol names because
*"a daemon crash report is worthless without function names"*. There is no panic hook in this
workspace, and a `--detach`ed daemon's stderr is the null device — so today the panic message goes
nowhere at all, and the one artifact those three sentences were written for was never built.

This task builds it, and settles the two adjectives in its own sentence.

## Goal

A `mixengined` that panics leaves two things behind: a line in `daemon.log` that says what happened,
and a small file that a person can attach to a public bug report **without reading it first** —
because nothing in it can be about them.

## Measured, not assumed

Read on 2026-09-05 out of this tree rather than reasoned about.

1. **There is no panic hook.** `grep -rn "set_hook\|catch_unwind\|PanicHookInfo" crates/` returns
   nothing outside three test names in `mixengine-platform`.
2. **A detached daemon's panic output is discarded.**
   `crates/mixengine-platform/src/process.rs:1443` (`spawn_detached`) sets `.stdin(Stdio::null())`,
   `.stdout(Stdio::null())`, `.stderr(Stdio::null())`, and `main.rs:425` is what starts the real
   daemon through it. The default panic hook writes to stderr and nowhere else.
3. **`api/rpc.rs:838` states the opposite**, in a comment explaining why the panic message is not
   repeated to the client: *"The panic message itself has already gone to the log through the panic
   hook."* It has not.
4. **A panicking handler is already contained**, and there is already a handler that panics on
   purpose to prove it: `#[cfg(test)] "daemon.__panic"` at `api/rpc.rs:802`, caught through
   `JoinError::is_panic` and answered `internal`. Containment works; the *record* is what is
   missing.
5. **`panic = "abort"` is forbidden and `strip = "debuginfo"` is set**, both in `Cargo.toml`'s
   `[profile.release]`, the second with the comment *"Debug info goes, symbol names stay: a daemon
   crash report is worthless without function names."* So a release backtrace has symbols and no
   `file:line`.
6. **`daemon.log` already carries paths a person chose.** `blueprints.rs:213` logs a blueprint
   file's path at `info!`; `extensions/install.rs:629` logs a half-install's directory at `warn!`.
   It is not, and has never been, an artifact free of project paths.
7. **The bundle refuses a redaction pass, in writing.** `mixengine-proto/src/bundle_api.rs`'s header:
   *"A filter layered on top would be a guess that a pattern matched — and worse than nothing,
   because it would invite the next reader to believe the log is filtered rather than clean."*
8. **`Part` has five variants, `Part::ALL.len() == 5` is asserted, and `MANIFEST_FORMAT == 1`.**
   `Part` travels on the wire inside `BundleReport::members`.
9. **`doctor::Doctor::report` runs seventeen checks in a fixed order**, and `Outcome` already has
   `Note` and `Skipped`, neither of which makes `mix doctor` exit non-zero —
   `only_a_problem_makes_the_report_unhealthy` in `doctor_api.rs`.
10. **`Paths` has twelve directories in `directories()` and one computed path beside them**,
    `service_logs(&ServiceId) -> PathBuf`, which is not among the twelve and is created on demand.
11. **Nothing in `mixengine-daemon`, `-supervisor` or `-core` names a thread.**
    `grep -rn "\.name(" … | grep -i "thread\|Builder"` is empty, so every thread name a panic hook
    could read is `main`, `tokio-runtime-worker` or `tokio-runtime-worker`-like — a literal from
    tokio, never a value from this home.
12. **Nothing has ever been released from this repository** — recorded by T89 in
    [phase-9-ship.md](../../../.claude/roadmap/phase-9-ship.md), and the reason it could decline to
    repair two destructive migrations.
13. **`config::Config` is `deny_unknown_fields`** and `config/template.toml` lists every key
    commented out; a test holds the two in step.
14. **`docs/guide/en/cli.md` is generated** from `mix docs --reference` and diffed by the `docs` CI
    job, and every `vi/` page carries a `source_sha256` over its English source, restamped by
    `bash packaging/docs.sh --restamp`.

## Scope

**In.** A panic hook in `mixengined`; a `CrashReport` type in `mixengine-proto`; the file it is
written to under `logs/crashes/`; a `[crash]` section in `config.toml`; an eighteenth `mix doctor`
check; a sixth bundle `Part`; the handbook paragraph in both languages; an ADR for the word
*opt-in*.

**Out.** Any transmission of anything, to anywhere. A crash-report *command* (`mix crash …`) — the
doctor names the file and the bundle carries it, and a third way to reach four fields is a third
thing to keep in step. A hook in `mix`, in the shim, or in `mixengine-elevate` (D2). The RPC method
name at the moment of the panic (D9, *what this leaves*). Minidumps, signal handlers, and anything
that would try to record a `SIGKILL`, an OOM kill or a hardware fault — a panic hook sees panics and
says so.

## What "opt-in" turns out to mean

The word was written against a design in which a crash report is uploaded. This build uploads
nothing: there is no endpoint, no client, no queue and no key, and adding one would contradict
[ADR 0017](../../../.claude/decisions/0017-smart-app-control-is-an-unsupported-configuration.md) and
[updates.md](../../../.claude/features/updates.md), both of which say in as many words that there is
no telemetry here and that T91's reporter *"is not an inventory of machines"*.

So the consent that "opt-in" is about is spent on a command a person types — `mix doctor --bundle` —
and the file on disk before that is a log, not a report. **Recording is on by default**, because a
crash you did not record is a crash nobody can fix, and a switch that must be thrown *before* the
first crash is a switch whose answer is always "no" at the moment it mattered. `[crash] enabled`
exists so that somebody who wants no file at all can have that, which is a stronger control than the
one the sentence asked for.

Argued at length, so it is not re-litigated:
[ADR 0022](../../../.claude/decisions/0022-a-crash-report-is-recorded-by-default-and-sent-by-nothing.md).

## Decisions

### D1 — The report carries no panic message, by construction

Every field of a `CrashReport` is one of three things: a **compile-time constant of this build**
(the panic location, the version, `std::env::consts::OS`/`ARCH`), a **literal from tokio or std**
(the thread name — measured, fact 11), or a **symbol name** from the backtrace. None of them can
hold a value from this home.

The panic *message* is the one part that can, because it is `format!`-ed from whatever was in scope:
an `unwrap()` on `mixengine_core::Error::Io` renders the path it carries, and an `expect` written
carelessly next year renders anything. ADR 0006 keeps credentials out of the spec, the database and
the log *at the type level* — it says nothing about a string built at the moment of a bug, which is
precisely the code path nobody reviewed.

So the message goes to `daemon.log` at `error!`, where it is on the user's own machine, and the
report does not carry it. This is the same move `bundle_api.rs` made and the opposite of a filter:
nothing is scanned, nothing is guessed at, and the guarantee is a list of fields rather than a
regular expression.

**What is lost** is the message when the report travels alone. It is small: the location is
`file:line:col` in this repository, so the reader opens the line — and for the `unwrap`/`expect`
panics that are nearly all of them, the line *is* the message. Whoever wants more asks for the
bundle, which carries the log.

### D2 — Only `mixengined` gets a hook

- **`mix`** and **the shim** run in a terminal a person is looking at. Their stderr *is* the report,
  delivered instantly, and the default hook already writes it. A file would be a second copy of
  something already read.
- **`mixengine-elevate`** is excluded and the reason is security, not tidiness: it runs as root, and
  a root-owned file created inside a directory an ordinary account can write is a symlink target
  waiting for one. It is also
  [excluded from auto-update](../../../.claude/features/updates.md), audited by hand, and has a
  dependency budget in `.github/elevate-dependencies.txt` that a hook would spend for nothing.
- **`mixengined`** is the one process with no screen, the one whose panic can take a supervision
  loop down silently, and the one three documents already promised this for.

A shared implementation is not built, and could not be placed anyway: `mixengine-cli` does not
depend on `mixengine-core`, `mixengine-proto` is types-only by its own header, and
`mixengine-platform` is for what differs per OS. A panic hook is none of those.

### D3 — Where the reports live: `logs/crashes/`

Under `logs/`, so that they follow a `[paths] logs` override onto a bigger disk, are removed with
the rest of `logs/` by `mix uninstall`, and sit beside `daemon.log`, which is the other half of the
same story. Not `cache/`, where the bundles are: a bundle is a thing you can always take again, and
a crash report is evidence of a moment that will not repeat.

`Paths::crashes() -> PathBuf` is computed, exactly as `Paths::service_logs` is, and is **not** added
to `directories()`: the directory is created on the first crash and an installation that has never
crashed has no such directory, which is a more useful thing for a person to find than an empty one.

**File name**: `crash-<unix-millis>-<pid>-<seq>.json`. Millis rather than the ISO form `Timestamp`
can already print, because that form contains `:` and Windows will not have it in a file name.
`<seq>` is a process-wide atomic counter and is not decoration — two threads panicking in the same
millisecond of the same process is exactly what a crash loop looks like, and without it the second
report overwrites the first.

**Written to `.tmp` and renamed.** A report the process was killed halfway through writing is a JSON
file that will not parse, and D7 would then have to decide what a bundle does about it. A rename is
one syscall and removes the question. Only `*.json` is ever read back; a `.tmp` left by a hard kill
is ignored and pruned with the rest.

### D4 — The hook: order, and what it must survive

```
panic
 └─ 1. build the report and write the file        (the evidence, first)
    2. call the previous hook                     (stderr — a developer's terminal)
    3. tracing::error!(location, thread, message) (daemon.log, inside the request's span)
```

**The order is about a deadlock**, and it is the one real hazard here. A panic hook runs *before*
unwinding, on the panicking thread, while every lock that thread holds is still held. The logging
sink is an `Arc<Mutex<RotatingFile>>`; a panic raised inside it would make step 3 lock a mutex this
thread already owns and hang the daemon. It is unlikely — `RotatingFile` returns its errors rather
than panicking — and it is not preventable from here, so the response is to put the write that
matters *first* and to name the hazard rather than pretend it away.

**Step 3 is inside the tracing span the request opened**, which is what puts `method` and
`request_id` on the line for free — the thing D9 declines to put in the file.

**Re-entrancy is guarded.** A thread-local `Cell<bool>` makes a panic raised *inside* this hook
delegate straight to the previous one instead of recursing. Every fallible step is `let _ = …`; the
hook has no `unwrap`, no `expect` and no indexing.

**Installed in `main`, immediately after `logging::init` and before the first `info!`.** A panic
before that point — argument parsing, resolving the home, reading `config.toml` — gets the default
hook, and that is correct: those failures happen while somebody is watching stderr, and none of them
has a log to be written to yet. The `--detach` parent returns before this line and installs nothing,
which is also correct: its stderr is the terminal the command was typed in.

`[crash] enabled = false` does not skip the hook — it skips step 1 alone. Steps 2 and 3 are logging,
which `rust.md` already promises and which this key has no business switching off.

### D5 — The backtrace: symbol names, and nothing that could be a path

`std::backtrace::Backtrace::force_capture()` — `force_`, so a report is never empty because
`RUST_BACKTRACE` was not exported into a daemon nobody launched by hand.

Its `Display` is the only access stable Rust offers, so the frames are extracted from it:

1. keep lines matching `^\s*\d+:\s+(.+)$` and take the capture; this drops every
   `at <path>:<line>:<col>` continuation line, which is where a build path lives;
2. **drop any frame that still contains `/` or `\`** — a Rust symbol never does, so this removes
   nothing legitimate today and keeps the guarantee true if the upstream format changes under us;
3. cap at 64 frames and 512 characters per frame, so one report is a few kilobytes and a crash loop
   cannot fill a disk.

Step 2 is the answer to the obvious objection — *you are parsing another crate's output format*. The
parse is best-effort and the guarantee does not rest on it. `rust.md` asks for a test rather than a
sentence where a claim is about somebody else's behaviour, so both are tested: a real
`force_capture()` in a debug build must yield at least one frame and no frame containing a
separator, and a synthetic sample carrying `at C:\Users\someone\project\src\main.rs:12:5` must lose
that line entirely.

### D6 — Surfaced by `mix doctor`, as a `Note` and never a `Problem`

An eighteenth check, *"crash reports"*, at the end of the fixed order:

| State | Outcome |
| --- | --- |
| `[crash] enabled = false` | `Skipped` — "crash reports are switched off in `config.toml`" |
| no reports | `Ok` |
| reports exist | `Note` — how many, when the newest was, where the directory is, and that `mix doctor --bundle` is what sends them |

**`Note` and not `Problem`, and this is not a stylistic choice.** `mix doctor` exits non-zero on a
`Problem`, and a crash recorded once would then make every `mix doctor` in every script fail
forever. It is also not a fault of the machine, which is what a `Problem` and its `ProblemId` are
for — so **no `ProblemId` variant is added**, and `daemon.doctor_repair` gains nothing to decline.

Nothing deletes reports: the cap of twenty bounds them, and a repair that threw away the evidence
somebody had not read yet would be the wrong kind of tidy.

### D7 — `Part::Crashes`, and the wire rule it forces to be written down

The bundle gains a sixth part, `crashes.json`: every report in `logs/crashes/`, newest first,
decoded and re-encoded as an array. A report that will not parse is skipped and named in `omitted`,
which is `diagnostics.rs`'s own rule — *"a part that could not be read is an omission and not the
end of the call"*.

It belongs here more than anything else does: the bundle's whole thesis is *one archive a person can
attach*, and the crash reports are the one member of it that is clean by construction.

**`MANIFEST_FORMAT` goes to 2.** A reader that knows format 1 and meets `"crashes"` in `parts` fails
to deserialise; the number exists so that such a reader stops with a sentence instead of a serde
error.

**And `Part` growing is a wire change that ADR 0019 does not cover.** That ADR is about a *member*
added to a response; this is a *variant* added to an enum that travels inside `BundleReport::members`,
and an older `mix` meeting it cannot decode the answer at all. It is free today for the reason T89
could decline to repair two destructive migrations — nothing has ever been released — and it will
not be free after v0.1.0. The rule is recorded in ADR 0022 rather than left to be rediscovered: after
the first release, a new `Part` bumps `PROTOCOL_VERSION`.

### D8 — `[crash]`, one key

```toml
[crash]
# MixEngine writes a small file when the daemon hits a bug in itself: where in MixEngine's own
# source it happened, the function names around it, and nothing else. It carries no file paths of
# yours, no site names and no passwords — by what it is allowed to contain, not by filtering.
#
# Nothing sends it anywhere. It is written to logs/crashes/ and stays there; "mix doctor --bundle"
# is the one thing that packs it into an archive, and that is a command you type.
#
# false stops the file being written. The daemon log still records that a crash happened.
#enabled = true
```

One key and not three. The retention cap is a constant (`KEPT = 20`) for the reason
`BUNDLES_KEPT = 3` is one in `diagnostics.rs`: it exists to bound a disk, not to be tuned. The
`[crash]` section is where a second key goes if one is ever earned.

### D9 — What the type is, and where it lives

`mixengine-proto`, a new `crash.rs`, for `bundle_api.rs`'s own reason: *"a bundle is read by whoever
was sent one, which is rarely the machine that produced it"* — so the shape lives in the crate every
client links, and `ts-rs` exports it with the rest (T56; `bash packaging/bindings.sh`).

```rust
pub const CRASH_FORMAT: u32 = 1;

pub struct CrashReport {
    pub format: u32,             // CRASH_FORMAT
    pub recorded_at: Timestamp,
    pub daemon: DaemonVersion,   // version + protocol, the frozen type
    pub os: String,              // std::env::consts::OS
    pub arch: String,            // std::env::consts::ARCH
    pub thread: Option<String>,  // None when the thread is unnamed
    pub location: Option<CrashLocation>, // None when std reports none
    pub frames: Vec<String>,     // symbol names, D5
}

pub struct CrashLocation { pub file: String, pub line: u32, pub column: u32 }
```

`file` is `PanicHookInfo::location()`'s `&'static str`, which is the path **as it was written in
this repository** (`crates/mixengine-daemon/src/…`) — a constant of the build, not a directory on
anybody's disk.

### D10 — Testing

Where the behaviour lives, per `testing.md`.

| Claim | Where |
| --- | --- |
| the frame filter drops `at …` lines, drops any frame with a separator, and caps | `mixengine-daemon`, unit, over a synthetic capture with a Windows path in it |
| a real `Backtrace::force_capture()` survives the filter with at least one frame and no separator | same module, unit |
| a report round-trips, and refuses an unknown field | `mixengine-proto`, unit |
| the hook writes exactly one file per panic, names it uniquely under concurrency, and prunes to twenty | `mixengine-daemon`, unit, against a `TempDir` — the writer is a plain function taking a directory, so the hook itself is a three-line closure over it |
| the report contains no message, whatever the panic said | `mixengine-daemon`, unit: panic with a payload holding `C:\Users\someone\secret`, assert it is in no field |
| `daemon.__panic` leaves a report on disk and the daemon stays up | `mixengine-daemon/tests/`, the suite that already drives that method |
| the doctor answers `Ok`, `Note` and `Skipped` in the three states | `mixengine-daemon`, unit |
| `Part::ALL.len() == 6`, names unique, manifest last | `mixengine-proto`, the existing test, updated |
| a bundle carries `crashes.json`, and an unparseable report becomes an `Omission` | `mixengine-daemon/tests/` beside the existing bundle suite |
| the template and the type agree about `[crash]` | `mixengine-core`, the existing config test |

No new CI job — T89's and T90's rule: this is `cargo test` with nothing to download and no privilege
to acquire, so it runs on all three runners with no edit to the workflow.

### D11 — The handbook

`docs/guide/en/troubleshooting.md` gains a short section — what the file is, where it is, what it
does *not* contain, and that nothing sends it — and `docs/guide/vi/troubleshooting.md` gains the
translation, restamped with `bash packaging/docs.sh --restamp`. No CLI flag changes, so
`docs/guide/en/cli.md` is untouched and the `docs` job's diff stays clean.

## What this leaves

- **A panic is not every way a daemon dies.** A `SIGKILL`, an OOM kill, a stack overflow and a
  hardware fault leave nothing, because a panic hook is not a signal handler and this task does not
  become one. `mix doctor`'s note says "crash reports", not "crashes".
- **The method that was being served is not in the file.** It is on the `error!` line in
  `daemon.log`, through the span, and putting it in the report as well would need a task-local
  scoped around every dispatch. Worth doing; not worth smuggling into this task.
- **The startup window is uncovered**: a panic before `logging::init` gets the default hook and
  stderr. Covering it would mean a hook installed before the home is known, which is a hook with
  nowhere to write.
- **Nothing announces a crash from the previous run at start-up.** The doctor is where it is
  surfaced, and a daemon that logged "the last run panicked" on every start until somebody deleted a
  file would be a daemon nagging about a file it will not delete.
- **The deadlock in D4 is accepted, not closed.** Writing the file first is what makes it survivable
  rather than silent.

## Risks

| Risk | Answer |
| --- | --- |
| The backtrace format changes upstream and frames stop being extracted | The report degrades to a location, which is most of its value; the separator guard means it cannot degrade into leaking a path. Two tests fail loudly first. |
| A crash loop writes twenty files and a log line per restart | Twenty files of a few kilobytes, capped; the log rotates at 10 MB as it already does. |
| Somebody later adds a field to `CrashReport` that carries a value from this home | D1 is the rule, ADR 0022 states it, and the field list is short enough to read in one screen. This is a review property, and it is the one thing here no test can hold. |
| `mix doctor` grows noisier | One line, only after a crash, and never an exit code. |
