# T40 — The one-shot elevated helper, and the protocol it answers

*Design, 2026-08-22. Roadmap task [T40](../../../.claude/roadmap/phase-4-sites-and-elevation.md), Phase 4.*

## What this closes

`crates/mixengine-elevate/src/main.rs` is thirty-six lines that refuse every request, and its
`Cargo.toml` has no dependencies at all. `PrivilegedOp` exists in
[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md#privileged-operations)
and nowhere in the workspace. Everything Phase 4 and the whole of Phase 5 do to a machine goes
through this binary, and today there is nothing there to go through.

T40 builds the frame: a typed request read from a file, validated by the elevated process itself,
applied, reported in a response file, recorded in an audit log the user cannot rewrite. Plus exactly
one operation — `Probe` — which touches nothing and exists for the three reasons in D1.

**Nothing reaches it yet.** The `Elevation` trait that raises the OS prompt is T40a; the queue that
batches pending operations behind one prompt is T40b; `HostsApply`, the first operation that changes
a machine, is T41. This is the same shape T20, T21 and T22 shipped in — the mechanism first, its
caller afterwards — and the reason is the same: a request protocol argued about while an RPC method
is being written is a protocol shaped by that method.

T40 is also the task that creates the `system` CI job.
[build-and-release.md](../../../.claude/operations/build-and-release.md) says that job arrives with
"the first `#[ignore]`d system test". This is it.

## What already exists, and is reused unchanged

- `mixengine_platform::lock` — the advisory lock, whose whole mechanism is a held handle rather than
  a file anyone has to clean up. Two overlapping elevation prompts are a thing a user can produce.
- `mixengine_platform::paths::in_full` — path comparison normalised on both sides, the rule T39/D5
  paid for. Every path this binary compares is a path it might be attacked through.
- `serde` and `serde_json`, already in `mixengine-proto` and already that crate's only wire
  vocabulary. No new dependency carries the protocol.
- The `DirectoryAccess` **shape** — a capability that applies permissions and can also report on
  them — though not its implementation, for the reason in D7.
- The workspace layering table in `crates/mixengine-proto/tests/workspace_layering.rs`, which already
  permits `mixengine-elevate` to depend on `platform` and `proto` and nothing else. No edge is added.

## Decisions

### D1 — The frame plus one operation, and that operation is `Probe`

An empty frame cannot be run. Every success path in this task would stay unexecuted until T41, on
every operating system, and the first time the request/response lifecycle ran for real would be
inside a task that is simultaneously learning what a hosts file marker block is.

`Probe` applies nothing. It answers four questions and writes them into the response:

- which version of the binary this is,
- whether it is in fact running with an elevated token,
- which operations this build knows how to apply,
- where its audit log is.

Three things depend on it. The lifecycle is proved end to end on all three systems inside T40.
[T41a](../../../.claude/roadmap/phase-4-sites-and-elevation.md) gets a real, complete binary to put in
front of Smart App Control — the half of that task that needs no code from this phase and should not
wait for it. And the third is the other half of D2: `Probe` **is** the version negotiation.

`mixengine-elevate` is excluded from auto-update and installed once into a root-owned directory
([security-model.md](../../../.claude/architecture/security-model.md)). The consequence is not a risk
but a certainty: an old helper will meet a new daemon, routinely, for as long as the product ships.
Without `Probe` the only way for a daemon to learn what the installed helper can do is to send a
batch and read the failures — which spends a prompt, the one resource ADR 0005 budgets, to learn a
fact.

### D2 — The response file is the protocol; the exit code is what to read when there is no file

The helper is started through `ShellExecuteEx`, `osascript` and `pkexec`, and the three differ in how
faithfully an exit code survives the trip. `do shell script … with administrator privileges` raises
an AppleScript error on a non-zero status rather than handing back a number, so a design that encodes
outcomes as exit codes is a design whose macOS leg has to reconstruct them from an error string.

The rule is one sentence: **if `response.json` exists, read it and ignore the exit code; only when
there is no file does the exit code mean anything.**

| Code | Meaning | Response written |
| --- | --- | --- |
| 0 | The batch was processed and reported | yes |
| 64 | The arguments made no sense — a caller bug | no |
| 65 | The request could not be read, parsed, or passed whole-request validation | no |
| 69 | This helper cannot run here | no |
| 70 | Internal failure | no |

Exit 0 **even when every operation failed**: the batch was processed and there is a report to read.
This inverts the current stub, which is right about the danger and wrong about the remedy — its
comment says the daemon must never read 0 for a request that was not applied, and the fix is that 0
means "there is a report", not "it worked".

Every code is at or below 125. `pkexec` reserves 126 and 127 for its own failures and shells use
128+n; a helper that spent those numbers would be indistinguishable from the launcher failing to
start it.

### D3 — Operations are decoded one at a time, and unknown fields are fatal

The request carries `ops` as a list of undecoded JSON values, not as `Vec<PrivilegedOp>`.

A `Vec<PrivilegedOp>` fails as a whole when it meets one variant this build has never heard of, which
by D1's argument is a routine event rather than a corruption. Decoding element by element turns an
unknown operation into an `Unsupported` outcome **at its own index**, leaves its neighbours applied,
and lets the daemon read exactly what did not happen.

Forward tolerance stops precisely there. Each operation is decoded with `deny_unknown_fields`: an
older helper must **not** silently ignore a field it does not recognise inside an operation it thinks
it understands. That is the path by which a weaker version of an operation gets applied and nobody
finds out — the failure mode is not a crash, it is a hosts file written without the constraint that
was supposed to bound it.

### D4 — The request file's owner is the identity

The helper runs as root and its caller does not. The rule the security model states is that the
daemon runs as the user and **if the daemon is compromised it is the attacker**, so nothing the
request asserts about who is asking can be believed.

It does not have to be. The daemon wrote the request file while running as the user, so the file's
owner *is* the calling identity — read from the filesystem, not from the document. No `PKEXEC_UID`,
no walking up to a parent process, no environment variable: three mechanisms that differ per OS and
two of which an attacker sets.

From that one fact the rest follows:

- The request file must not be a symlink, must not be owned by root, and must not be writable by
  others.
- `home` in the request must be a directory owned by that same identity, and the request file must
  lie inside it. Skipping this makes `--home C:\Windows\System32` an escalation for every operation
  that takes a path.
- Every path in every operation is canonicalised and must resolve inside that `home`.
- The whole run is wrapped in the advisory lock.

### D5 — Elevation is a property of the operation, not a gate on the process

The obvious frame refuses to do anything at all when it is not running elevated. It is wrong, and
`Probe` is what shows it: the operation whose job includes reporting whether the token is elevated
would then be unable to ever report `false`.

So `PrivilegedOp::requires_elevation()` is the gate, applied by the frame at one place — still one
line for an auditor to find — and answered `false` by `Probe` and `true` by everything else. An
operation that needs a privilege the process does not hold is `Refused` at its own index.

Two things fall out. `Probe` runs under an ordinary token, so the request/response lifecycle is
covered by the existing `test` job rather than only by the elevated one. And the assertions that
genuinely need a full token are the only ones left in `system`, which is the distinction
[testing.md](../../../.claude/standards/testing.md) draws: prove a privilege claim by reading a fact,
never by attempting an action the token gets to decide.

### D6 — `Probe` joins the closed list, and the list is closed against powers

[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md#privileged-operations)
says the list is closed and that adding to it requires an ADR. `Probe` is an addition.

The rule exists to stop a new capability being granted quietly. `Probe` grants none: it is the only
member that changes nothing, and the only one answering `requires_elevation() == false`. So the
document's wording is made precise — the list is closed against operations **with effects**, and a
non-mutating self-report is not one — rather than an ADR being written to record that nothing was
decided. Removing an entry already needs no ADR, for the symmetric reason, and T26 already did it.

### D7 — The audit log lives outside `MIXENGINE_HOME`, and it is evidence rather than a defence

[security-model.md](../../../.claude/architecture/security-model.md) says "root-owned, append-only
`logs/elevate.log`". `logs/` is inside `MIXENGINE_HOME`, which the user owns and writes. A root-owned
file in a user-owned directory can be renamed or unlinked by that user whatever its own mode says, so
"append-only" there is a promise the filesystem does not keep.

| OS | Path | Permissions |
| --- | --- | --- |
| Windows | `%ProgramData%\MixEngine\elevate.log` | `Administrators` and `SYSTEM` full, `Users` read, inheritance severed |
| macOS | `/Library/Logs/MixEngine/elevate.log` | root-owned, `0644` |
| Linux | `/var/log/mixengine/elevate.log` | root-owned, `0644` |

The directory is created **by the helper on first run**, not by the installer, because the helper has
to work on a machine no installer has touched — which is exactly the machine T41a runs it on. If the
path already exists and is not a root-owned directory, or is a symlink, the run is refused: a
directory root writes into is itself a target.

Format is JSON Lines, one line per operation per invocation: timestamp, helper version, the calling
identity taken from D4's file owner rather than from the document, the nonce, the operation and its
arguments in summary, and the outcome. The timestamp is **epoch milliseconds**, not a formatted date:
rendering a calendar date needs a date library, and adding one to a binary that runs as root to make a
log line prettier is not a trade this budget makes. Whoever reads the log has a daemon and a `mix
doctor` that can format it. Appended with `O_APPEND` / `FILE_APPEND_DATA` and never
written by the atomic replace the rest of the codebase uses — replacing the file whole is the one
thing that would destroy the property this file exists to have.

No rotation. A machine produces a few dozen lines over the lifetime of an installation, and rotation
here is code running as root to solve a problem that does not occur.

**And it makes what ran readable, nothing more.** It prevents nothing, and specifically it does not
prevent the binary-replacement path in the threat table below — a helper that has been replaced is
also the thing writing the log.

**A debt this creates:** the log is the first thing MixEngine leaves outside `MIXENGINE_HOME`.
Removing it is itself a privileged operation, so `mix uninstall` owes it one; that belongs to T47 and
T92, and this task records the obligation in the phase file rather than discharging it.

### D8 — `mixengine-platform` grows features, and the elevated binary's dependency closure is checked

The layering table permits `mixengine-elevate` to depend on `mixengine-platform`. Today that crate
carries `tokio`, `keyring` (with a vendored libdbus on Linux), `directories` and a broad `windows-sys`
feature list, and all of it would land in a binary whose own `Cargo.toml` says "keep this dependency
list as short as it can possibly be: everything here runs as root".

So the crate gains `[features]`:

| Feature | Pulls | Used by |
| --- | --- | --- |
| `elevated` | `libc` on unix, a narrow `windows-sys` set on Windows | the helper alone |
| `ipc`, `process`, `signal` | `tokio` | daemon, supervisor, CLI |
| `keyring` | `keyring` | core, daemon |
| `home` | `directories` | nearly everything |
| `default` | all of the above | every dependent today, unchanged |

`default` covering everything is what keeps this from being a workspace-wide edit: not one existing
dependent changes a line. The helper declares `default-features = false, features = ["elevated"]`.

`elevated` is a small per-OS module holding the primitives that only mean anything under an elevated
token: am I elevated, who owns this file, create a directory root owns and everyone reads, replace a
file atomically. The third is genuinely new — `DirectoryAccess::restrict_to_owner` answers the
opposite question, keeping other accounts *out*, whereas here root owns the directory and the user
must be able to read it.

**The lean tree is checked, not trusted.** Cargo unifies features across a workspace build, so
`cargo build --workspace` still compiles `platform` with tokio and keyring and links the helper
against that. The guarantee holds only when the helper is built on its own. So `lint` gains a step
that runs `cargo tree -p mixengine-elevate --no-default-features` and diffs it against a committed
list, and the release pipeline builds the helper in an invocation of its own. Growing that list by a
line is a security decision, which is what the comment in the helper's manifest already claims and
what this makes true.

### D9 — No shell strings, anywhere, and the sharpest edge is macOS

Later operations call system programs — `resolvectl`, `security`, `update-ca-certificates`. Every one
of those calls goes out as an argv array with no shell interpreter, and no user-supplied data reaches
a command line. The rule is written here, in the task that builds the frame, because T41 onward is
where it becomes possible to break.

Recorded here for T40a rather than left to be rediscovered: on macOS the prompt is raised by
`do shell script … with administrator privileges`, which takes a **string**. The path to the helper
and its one argument are interpolated into an AppleScript string literal, and a quoting mistake there
is arbitrary code as root. It is the single most dangerous line in the elevation path on any of the
three systems.

### D10 — Anti-replay is the existence of the response file

The helper refuses a request whose sibling `response.json` already exists. A processed request cannot
be processed twice, the check costs one `stat`, and it needs no clock, no nonce store and no state
that outlives the process. The daemon writes each request into a fresh single-use directory under
`run/elevate/<id>/`, so the response is the only thing that ever appears beside it.

The nonce in the document is a different guard against a different mistake: it is echoed into the
response so a daemon cannot take the answer to an earlier request for the answer to this one.

### D11 — "User declined" is not an exit code of ours

A declined prompt is a normal outcome, and the helper cannot report it: when the user clicks Cancel
the helper never ran. It is therefore an `ElevationOutcome::Declined` in the protocol vocabulary, not
a number in D2's table, and mapping `ERROR_CANCELLED` (1223), osascript's `-128` and `pkexec`'s 126
onto it belongs to T40a where the three launchers live. T40 defines the word so that T40a has one to
use.

## The protocol

In `mixengine-proto`, a new `privileged` module. Types only; no I/O, which is the rule that crate
already keeps.

```rust
pub const PROTOCOL_VERSION: u32 = 1;

pub struct PrivilegedRequest {
    pub version: u32,
    pub home: PathBuf,
    pub nonce: String,
    pub ops: Vec<serde_json::Value>,   // decoded one at a time — D3
}

#[serde(tag = "op", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PrivilegedOp {
    Probe,
    // HostsApply arrives with T41; the rest with T42, T44, T45 and Phase 5.
}

pub struct PrivilegedResponse {
    pub version: u32,
    pub elevate_version: String,   // the binary's own semver, not the protocol's
    pub nonce: String,
    // The report, carried by every response rather than by one operation's outcome — see below.
    pub elevated: bool,
    pub supported_ops: Vec<String>,
    pub audit_log: PathBuf,
    pub results: Vec<OpOutcome>,
}

pub enum OpOutcome {
    Applied { detail: String },
    AlreadyDone,
    Refused { reason: String },      // validation failed — the caller's fault
    Unsupported { reason: String },  // this build does not know this operation
    Failed { message: String },      // the OS refused
}

pub enum ElevationOutcome {   // T40a's vocabulary, defined here — D11
    Completed,
    Declined,
    Unavailable { reason: String },
}
```

**The report is a property of the response, not the outcome of `Probe`.** Squeezing it into
`Applied { detail }` would mean a JSON document nested inside a JSON string, and would mean the daemon
learns what the installed helper can do only on the round trips where it thought to ask. In the header
it costs a few strings, arrives on **every** answer, and is read the same way whatever the request
contained. `Probe` is then simply the operation that applies nothing — what a caller sends when the
report is the only thing it wants, since a request with an empty `ops` list asks for nothing and is
refused as malformed rather than given a meaning of its own.

The daemon builds a `Vec<PrivilegedOp>` and serialises it into the request's `ops`; the asymmetry with
the undecoded `Vec<Value>` on the reading side is D3's whole point, and is confined to that one field.

The helper is invoked with **one** argument, the path to the request file. The response path is not
passed: it is `response.json` beside the request. One fewer argument is one fewer thing to validate,
and the daemon already knows where to look.

## Crate changes

**`mixengine-proto`** — new `privileged` module. No new dependency.

**`mixengine-platform`** — `[features]` per D8, with `default` enabling all of them; existing
dependents unchanged. New `elevated` module with the four per-OS primitives, behind that feature.

**`mixengine-elevate`** — the binary. `main.rs` shrinks to argument handling and exit codes; the work
lives in modules that are testable without root: `request` (read, validate, decode per D3 and D4),
`audit` (the log per D7), `ops` (dispatch and `Probe`). Dependencies: `mixengine-proto`,
`mixengine-platform` with `default-features = false, features = ["elevated"]`, and `serde_json`. Not
`serde` — the derives live on the types, which live in proto, and this crate only parses and writes
what proto already describes.

**No change** to `mixengine-daemon`, `mixengine-core`, `mixengine-cli` or the GUI. Nothing calls this
binary until T40b.

## Testing

**Unit, in the helper.** Validation is pure functions: parsing a request, decoding one operation,
`requires_elevation`, building a log line. No OS involved.

**Integration under an ordinary token**, `crates/mixengine-elevate/tests/protocol.rs`, inside the
existing `test` job. It runs the real binary. The distinction the assertions turn on is D2's: a
**whole-request** refusal exits 65 and writes no response, while a **per-operation** refusal exits 0
and is reported at its index.

| Given | Exit | Response |
| --- | --- | --- |
| no arguments | 64 | none |
| a request path that does not exist | 65 | none |
| malformed JSON, or a `version` this build does not know | 65 | none |
| a sibling `response.json` already there (D10) | 65 | the old one, untouched |
| a request file owned by somebody else, or a symlink (D4) | 65 | none |
| `home` not owned by that same identity (D4) | 65 | none |
| an unknown operation beside a known one (D3) | 0 | `Unsupported` at its index, the neighbour applied |

And `Probe` runs to completion, because D5 lets it.

The trap here is the one [testing.md](../../../.claude/standards/testing.md) names: the Windows leg of
`test` runs under a **full administrator token** (T2b), so an assertion phrased "refused because the
token is not elevated" would be red there and green elsewhere for reasons that have nothing to do
with the code. The assertion is therefore about consistency with what `Probe` reports — elevated
implies `Applied`, not elevated implies `Refused` — which reads the same from any token.

**The `system` job, which this task creates.** Elevated, on all three runners, running the
`#[ignore]`d suite: the audit directory created at its real system path, a second run appending
rather than replacing, and the directory's permissions asserted **structurally** — reading the DACL
the way `crates/mixengine-platform/tests/access.rs` already does, never by attempting an access an
elevated process is allowed to make anyway. Triggered on `master`, and on a requested run whose
branch touches `platform` or `elevate`.

**A `lint` step** for D8's dependency budget.

**What T40 does not prove**, so that nobody reads the green as covering it: nothing tests raising an
OS prompt (T40a), and there is no "unrelated lines survive" regression test, because the rule in
[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md) is about editing a
system file and this task edits none except its own log. That test arrives with T41.

## The threat model this frame answers to

Stated because the frame's shape is an answer to it, and because
[security-model.md](../../../.claude/architecture/security-model.md) is explicit that some of it is
accepted rather than solved.

| Path | What an attacker gets | What stands in the way |
| --- | --- | --- |
| Replacing the helper binary | root at the next approved prompt | Installed to a root-owned directory, excluded from auto-update. **Accepted, not eliminated** — ADR 0005 says so, and calls it the trust model of `sudo` on a personal machine. |
| Writing a request and getting the user to approve | exactly the operations in the closed list | No `Exec`, no command, no script, no arbitrary path. This is what the list being closed is for. |
| `TrustCaInstall` with the attacker's CA | MITM of every TLS connection on the machine | The heaviest entry here. Every checkable constraint — CN, `pathlen:0`, key usage, validity — is one an attacker can reproduce, so the only thing between it and the machine is the user approving a prompt. Named rather than argued away. |
| Injection through the launcher | arbitrary code as root | D9, and the macOS string interpolation it singles out. |
| Symlink or TOCTOU on the request file or the log directory | a root-owned write to a file of the attacker's choosing | D4 and D7: refuse symlinks, check ownership, operate on handles. |
| A dependency inside a root binary | whatever that dependency can do | D8's budget, enforced by CI rather than by intention. |

The proportion worth keeping in view: an attacker already running as the user **can raise their own
prompt for their own program**. What the helper adds is a prompt that carries MixEngine's name — a
social-engineering advantage, real but modest — against a design that previously handed out root with
nobody having to click anything.

## Out of scope, and where each goes

| Not here | Where |
| --- | --- |
| Raising the OS prompt on the three systems | T40a |
| Batching pending operations behind one prompt, the `ElevationRequired` event, degraded mode | T40b |
| `HostsApply` and its marker block | T41 |
| Whether an unsigned build loads under Smart App Control | T41a, which this task hands a real binary |
| Removing the audit log at uninstall | T47 / T92 |
| Whether `mixengined` should refuse to start under an elevated token | recorded in the phase file; it is a change to the daemon, not to the helper |
