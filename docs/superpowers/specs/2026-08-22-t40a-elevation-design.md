# T40a — Raising the prompt: the `Elevation` trait and its three launchers

*Design, 2026-08-22. Roadmap task [T40a](../../../.claude/roadmap/phase-4-sites-and-elevation.md), Phase 4.*

## What this closes

[T40](2026-08-22-t40-elevate-design.md) built the helper: a typed request, validated by the elevated
process itself, applied, reported in a response file, recorded in an audit log the user cannot
rewrite. Nothing raises it. `Elevation` is a row in the trait table of
[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md#the-traits) and
appears nowhere in the workspace, and `ElevationOutcome` sits in `mixengine_proto::privileged`
carrying a doc comment that says "used by T40a" and no user at all.

T40a is the one capability that turns a request file lying on disk into an elevated process reading
it: `ShellExecuteEx` with the `runas` verb, `do shell script … with administrator privileges`,
`pkexec` — and, on the third, the polkit-agent gap
[ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md) calls the worst failure mode of
the three.

**Nothing calls this either.** The queue that batches pending operations behind one prompt, the
`ElevationRequired` event and degraded mode are T40b. This is the same shape T20, T21, T22 and T40
shipped in — the mechanism first, its caller afterwards — and the reason is the same: a capability
argued about while a queue is being written is a capability shaped by that queue.

## What already exists, and is reused unchanged

- `mixengine_proto::privileged::ElevationOutcome` — T40/D11 defined the word so that this task would
  have one. `Completed`, `Declined`, `Unavailable { reason }`, and the note that `ERROR_CANCELLED`,
  osascript's `-128` and `pkexec`'s `126` all land on the middle one.
- `PrivilegedRequest`, `PrivilegedResponse` and `RESPONSE_FILE_NAME` — named here and touched by
  nothing in this task, for the reason in D1.
- The `Host` bundle trait and `mock::Host`. Both carry five capabilities today; this is the sixth on
  each, and no existing pattern changes.
- The workspace layering table in `crates/mixengine-proto/tests/workspace_layering.rs`, which already
  permits `mixengine-platform` to depend on `mixengine-proto`. The edge is allowed and has never been
  taken; D8 takes it.
- The `system` CI job, created by T40, already elevated on all three runners.

## Decisions

### D1 — The trait stops at the prompt

```rust
fn run(&self, helper: &Path, request: &Path) -> Result<ElevationOutcome>;
```

It raises the prompt, waits for the process to end, and answers with one of three words. It does not
open `response.json`.

Reading the response is file I/O and `serde_json` over types `mixengine-proto` already describes:
there is no operating system anywhere in it. A capability is a question about the machine, asked of
an injected object so that a test can answer it from memory — and a mock that has to fabricate a
whole `PrivilegedResponse` is a mock of something that is not the OS. T40b reads the file, where the
types live one crate away and the reading is ordinary code.

**The consequence is worth stating rather than discovering.** `Completed` means the helper *ran*, not
that it left a report. A helper that died before writing one is `Completed` with nothing beside the
request, and T40b has to handle that state — which it has to anyway, on every system, because a
crash is not a per-OS event.

### D2 — The signature is two paths, which is D9 of T40 turned into a type

T40/D9 wrote the rule: no shell strings, anywhere, and every call out to a system program goes as an
argument vector. A launcher is precisely where that rule is easiest to break, because two of the
three mechanisms take a **string** and not a vector.

So the trait does not accept a command, an argument list, or anything a caller composes. It accepts
the path to the helper and the path to the request, and each launcher composes its own invocation
from those two values and constants of its own. There is no parameter through which caller-supplied
text could reach a command line, which makes the rule a property of the type rather than something
each of the three implementations has to remember.

The helper's location is the caller's to know: it is installed once, into a root-owned directory the
installer chose, and a platform layer guessing at it would be guessing at a decision T85 has not made
yet. What this layer does with it is check it — absolute, existing, a file — before handing it to a
mechanism that will run it as root.

### D3 — macOS: the AppleScript is a constant, and the paths arrive through `argv`

T40/D9 singles this out: `do shell script … with administrator privileges` takes a string, the helper
path and its argument are interpolated into an AppleScript string literal, and a quoting mistake
there is arbitrary code as root. It calls it the single most dangerous line in the elevation path on
any of the three systems.

The remedy is to stop interpolating:

```
osascript -e 'on run argv' \
          -e 'do shell script (quoted form of (item 1 of argv)) & " " \
              & (quoted form of (item 2 of argv)) with administrator privileges' \
          -e 'end run' \
          /abs/path/mixengine-elevate /abs/path/request.json
```

The script source is a compile-time constant with no value of ours anywhere in it. The two paths
travel as `argv` — an argument vector, which is what the rule asked for — and `quoted form of` is
AppleScript's own shell-quoting operator, which is correct about spaces, quotes and newlines because
that is the one thing it exists to be correct about.

The alternative is to escape twice by hand, once for the AppleScript literal and once for the shell
underneath it, and to keep being right about it in every future edit. That is maintaining, by hand, a
thing the platform already gets right.

Both paths are absolute, so neither can be mistaken for an option by `osascript`'s own argument
parsing.

### D4 — Windows: the quoting argument here is provable rather than conventional

`ShellExecuteExW` takes `lpParameters` as a single string, so the request path must be quoted. On
this system that is not a convention that mostly holds: `"` is an illegal character in a Windows path
name, so wrapping the argument in quotation marks cannot be defeated by a path — there is no path
that contains the one character that would end the quoting early. The check that makes it a proof
rather than a belief is a refusal: a path containing `"` is rejected before the call, so the
guarantee is enforced instead of assumed.

**And `lpDirectory` is set to the helper's own directory**, which is root-owned, rather than left
null. A null working directory means the elevated child inherits the caller's, and the caller is a
daemon whose current directory the user controls; Windows searches the working directory when it
resolves a DLL. An elevated process with its working directory in a user-writable place is a
DLL-planting target, and the fix costs one field.

Flags: `SEE_MASK_NOCLOSEPROCESS` for the handle to wait on, `SEE_MASK_NOASYNC` because the calling
thread has no message loop, and `SEE_MASK_FLAG_NO_UI` so that a failure comes back as an error code
rather than as a dialog on a machine where nobody is looking. Then `WaitForSingleObject` and
`GetExitCodeProcess`. A declined prompt is `ShellExecuteExW` returning false with `GetLastError` set
to `ERROR_CANCELLED` (1223) — the process never started, which is exactly why T40/D11 said this could
not be an exit code of the helper's.

### D5 — Linux: ask before spawning, and forbid the tty fallback

Two mechanisms, and the second is the one that closes ADR 0005's worst gap.

**`probe()` runs before anything is spawned.** `pkexec` on `PATH`, a session bus
(`$DBUS_SESSION_BUS_ADDRESS`, or a `bus` socket under `$XDG_RUNTIME_DIR`), and a graphical session
(`$DISPLAY` or `$WAYLAND_DISPLAY`). Any of the three missing is `Unavailable`, and the `reason`
carries the complete command for the user to run by hand.

**And the spawn passes `--disable-internal-agent`.** Without it, a `pkexec` that finds no
authentication agent falls back to the textual agent built into itself, which prompts on the
controlling terminal — a terminal a daemon does not have and could not show anyone if it did. With
it, `pkexec` fails immediately instead. The environment-based probe above is a heuristic and can be
wrong; this flag is what makes being wrong cheap, because the failure is a fast non-zero exit rather
than a process waiting forever on a tty nobody is watching.

`126` is a dismissed dialog and maps to `Declined`; `127` is "not authorized, or something went
wrong" and maps to `Unavailable`. Those are the two numbers T40/D2 kept the helper's own exit codes
below 125 to stay clear of, and this is the task that spends them.

**No polkit action file is shipped.** `pkexec` run against a program with no registered action falls
back to `org.freedesktop.policykit.exec`, which asks for an administrator password and caches the
credential briefly — the same shape as the other two systems. Installing a `.policy` file into
`/usr/share/polkit-1/actions/` is itself a privileged operation, which would mean needing elevation
in order to be able to elevate.

### D6 — `probe()` is a cheap pre-check, and only one system has a question worth asking before it spawns

```rust
fn probe(&self) -> ElevationSupport;   // Available | Unavailable { reason }
```

Rule 3 of [platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md#rules) —
detect, then act — and the caller it is for is `mix doctor` (T47) and T40b's degraded mode, both of
which need to say "this machine cannot elevate" without raising a prompt to find out.

Its honesty is not the same on the three systems, and pretending otherwise would be worse than the
asymmetry. Windows answers `Available` unconditionally: UAC is always there, and on an account with
no administrative rights the prompt asks for someone else's credentials rather than being absent.
macOS answers `Available` when `osascript` is there, which is nearly a constant; the case it cannot
see is a session with no window server, and detecting *that* cheaply and correctly is not something
this task has a way to do. Linux is the system where the question has a real answer, is asked, and
decides whether anything is spawned at all.

So `probe()` is documented as what it is: a cheap way to find out that elevation is impossible, never
a promise that it is possible. The authoritative answer is `run()`'s, and `run()` returns
`Unavailable` too.

### D7 — The helper's exit code does not enter the trait's vocabulary

Windows and Linux hand it back cheaply. macOS does not: `do shell script` raises an AppleScript error
on a non-zero status rather than returning a number, so the code has to be recovered from the text of
an error message.

Carrying it as `Completed { exit_code: Option<i32> }` would be an asymmetry that invites a caller to
depend on a number that is present on two systems and absent on the third — which is the exact
failure T40/D2 shaped the whole protocol around avoiding, and the reason it said the response file
*is* the protocol. So the trait answers with three words, and each launcher **logs** the code where
its OS gives one. A run that produced no report is diagnosed from the log by a person, not branched
on by the daemon.

The mapping each launcher does apply is only ever onto the three words: `ERROR_CANCELLED`, `-128` and
`126` onto `Declined`; a launcher that could not start at all onto `Unavailable`; everything else onto
`Completed`, because the helper ran and whatever it thought is beside the request.

### D8 — `ElevationOutcome` comes from proto, and the new edge is gated behind `host`

`mixengine-platform` may depend on `mixengine-proto` and never has. This task takes that edge, because
the type it needs is already there with a comment saying it was put there for this task, and because
the word for a declined prompt is one the daemon will put on the wire in T40b.

The dependency is **optional and enabled by the `host` feature**. `mixengine-elevate` takes this crate
with `default-features = false, features = ["elevated"]` and does not compile the trait; its closure
already contains `mixengine-proto` for the protocol itself, so `.github/elevate-dependencies.txt` does
not change either way. Gating it keeps the two halves of this crate honest about what each of them
needs, which is the whole point of T40/D8.

`windows-sys` gains `Win32_UI_Shell` and `Win32_System_Com`. Features on that crate add modules and
not crates, so nothing about the helper's dependency budget moves.

### D9 — The per-OS files are `prompt.rs` and not `elevation.rs`

`src/windows/`, `src/macos/` and `src/linux/` each already contain an `elevated.rs`: T40's helper-side
primitives — am I elevated, who owns this file, where does the audit log go. Adding `elevation.rs`
beside it would put two files one letter apart in one directory, on opposite sides of the privilege
boundary. The trait is `Elevation` and its file is `traits/elevation.rs`, in a different directory;
the implementations are `prompt.rs`, which is what they do.

## The interface

In `mixengine-platform`, behind the `host` feature.

```rust
/// Whether this machine can raise an elevation prompt at all.
pub enum ElevationSupport {
    Available,
    Unavailable { reason: String },
}

pub trait Elevation: std::fmt::Debug + Send + Sync {
    /// Can a prompt be raised here? Cheap, spawns nothing, and never a promise that it can — D6.
    fn probe(&self) -> ElevationSupport;

    /// Run `helper` once, elevated, with `request` as its only argument.
    ///
    /// Blocking, and with no deadline: a person reading a prompt is not a clock the OS gives us.
    /// The caller owns cancellation — T40b runs this on `spawn_blocking`, the way a keyring read is
    /// already run.
    fn run(&self, helper: &Path, request: &Path) -> Result<ElevationOutcome>;
}
```

and one more accessor on `Host`:

```rust
/// Raising the OS elevation prompt on the one-shot helper.
fn elevation(&self) -> &dyn Elevation;
```

`Error::UnsupportedPlatform` is not how an absent mechanism is reported here. A machine with no way
to prompt is a normal outcome that the daemon degrades around, which is what `ElevationOutcome`
already says; `Err` is reserved for a launcher that could not be *attempted* — a helper path that is
not an absolute existing file, a `"` in a path on Windows, an OS call that failed for a reason that
is not the user.

## Crate changes

**`mixengine-platform`** — `traits/elevation.rs` and the sixth accessor on `Host`;
`windows/prompt.rs`, `macos/prompt.rs`, `linux/prompt.rs`; `mock/elevation.rs`. An optional
`mixengine-proto` dependency enabled by `host`, and two more `windows-sys` features.

**No change** to `mixengine-proto` — `ElevationOutcome` is used as it stands.

**No change** to `mixengine-daemon`, `mixengine-core`, `mixengine-cli`, `mixengine-elevate` or the
GUI. Nothing calls this until T40b.

## Testing

**Unit, in `platform`.** Windows argument quoting and the `"` refusal. The exit-code and error-text
classification of all three launchers, as pure functions over a code and a stderr string, so that
each system's table is tested on every system rather than only on its own. Linux's `probe()` against
a constructed environment, in all four of its states.

**`mock::Elevation`.** Records the `(helper, request)` pairs it was asked to raise and answers with a
scripted outcome, in the style the existing mocks already use: `mock::Host::declining_elevation()`,
`mock::Host::unable_to_elevate(reason)`. This is the surface T40b's tests run against, and the reason
the trait is behind `Host` at all rather than being a free function like `lock` or `signal`.

**The `system` job**, which T40 created and which is already elevated on all three runners.

| Runner | What it proves |
| --- | --- |
| Windows | the whole round trip. This leg already holds a full administrator token, so `runas` raises no prompt: the launcher runs the real helper against a real request, `response.json` appears beside it, the outcome is `Completed` |
| Linux | `probe()` answers `Unavailable` and the reason carries the manual `pkexec` command. The runner has no graphical session, so this is not a second-class assertion — it is ADR 0005's worst branch, asserted on a machine that genuinely has no agent |
| macOS | the whole round trip under `sudo`, behind a hard timeout |

**The macOS row is a measurement and is written as one.** Whether `do shell script … with
administrator privileges` runs straight through when the process is already root, or authenticates
anyway, is not something this project can settle by reading — and CI is the only macOS it has. So the
test is written, run, and if it turns out to prompt, that leg is reduced to `probe()` and the finding
is **recorded** rather than guessed at in advance. This is T29's method applied to a yes/no question:
measure it, do not reason about it.

**What no CI run proves, stated so that green is not read as covering it:** nobody clicks Cancel. The
three constants that map onto `Declined` — 1223, `-128`, 126 — are read from documentation and held
by unit tests, and are confirmed only by a person sitting at a machine. T41a already needs a clean
Windows VM and is the natural place to confirm that leg while it is there.

## Out of scope, and where each goes

| Not here | Where |
| --- | --- |
| Batching pending operations behind one prompt, `ElevationRequired`, degraded mode, the "no code path elevates in a loop" test | T40b |
| Reading and validating `response.json` | T40b |
| Whether `mixengined` should refuse to start under an elevated token | T40b; recorded by T40 in the phase file |
| Where the helper is installed, and who puts it there | T85 / T86; this task takes the path as an argument |
| Whether an unsigned launcher and an unsigned helper run under Smart App Control | T41a |
| A confirmed observation of a real user declining a real prompt | T41a for Windows; unowned for the other two |
