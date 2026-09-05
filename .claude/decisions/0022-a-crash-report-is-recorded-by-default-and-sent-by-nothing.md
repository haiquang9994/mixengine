# 0022. A crash report is recorded by default and sent by nothing

**Status**: Accepted
**Date**: 2026-09-05

## Context

Roadmap task **T91** asks for *"crash reporting that is opt-in and contains no project paths or
credentials"*. Both halves of that sentence had to be decided rather than implemented, and neither
answer is the obvious one.

**"Opt-in" was written against a design that uploads**, and this product has nowhere to upload to.
[ADR 0017](0017-smart-app-control-is-an-unsupported-configuration.md) and
[../features/updates.md](../features/updates.md) both say, in as many words, that there is no
telemetry here and that T91's reporter *"is not an inventory of machines"* — each while declining to
count a population, because nobody here can take that reading. Building an endpoint to consent to
would contradict two accepted documents in order to satisfy one adjective.

**"Contains no project paths" could not be a claim about the log.** `daemon.log` carries paths a
person chose and always has: `blueprints.rs:213` logs a blueprint file's path at `info!`,
`extensions/install.rs:629` a half-install's directory at `warn!`. So the guarantee has to live in
something that is not the log — which is what makes a crash report a separate artifact rather than a
log line with more structure.

**And three documents already described a crash report that did not exist.**
[../standards/rust.md](../standards/rust.md) says the RPC layer turns a panic into `internal`;
`api/rpc.rs` says the panic message *"has already gone to the log through the panic hook"*;
`Cargo.toml`'s release profile keeps symbol names because *"a daemon crash report is worthless
without function names"*. There was no panic hook in the workspace, and `spawn_detached` gives the
real daemon `Stdio::null()` for its stderr — so the message went nowhere at all, and the comment in
`api/rpc.rs` was true of nothing.

## Decision

**A crash report is recorded by default, and it is transmitted by nothing.**

1. **There is no transmission.** No endpoint, no client, no queue, no key. `mixengined` writes
   `logs/crashes/crash-<millis>-<pid>-<seq>.json` and stops. The only act that can carry one off the
   machine is `mix doctor --bundle`, which is a command a person types — and that is where the
   consent the word *opt-in* was asking for is spent.

2. **Recording is on by default**, and `[crash] enabled = false` turns the file off. A crash nobody
   recorded is a crash nobody can fix, and a switch that has to be thrown *before* the first crash is
   a switch whose answer is always "no" at the moment it mattered. The key is a stronger control than
   the sentence asked for, not a weaker one: it withholds the file itself rather than a transmission
   that does not happen.

3. **What makes recording safe is the field list, not a filter.** Every field of a `CrashReport` is a
   compile-time constant of the build that wrote it (the panic's `file:line:col`, the version, the
   target), a literal from `std` or `tokio` (the thread name — nothing in this workspace names a
   thread), or a **symbol name** out of a backtrace with every `at <path>:<line>` line dropped. A
   frame that still contains a path separator is dropped too, which a Rust symbol never is — so the
   promise survives a change to `std`'s rendering.

4. **The panic message is deliberately absent from the report** and present in `daemon.log`. It is
   `format!`-ed from whatever was in scope at the moment of a bug, which is the one string in this
   daemon nobody reviewed: an `unwrap()` on `mixengine_core::Error::Io` renders the path that error
   carries. So it goes where the paths a person chose already are.

5. **After v0.1.0, a `Part` added to `bundle_api.rs` bumps `PROTOCOL_VERSION`.**
   [ADR 0019](0019-an-added-response-member-is-optional.md) settles an added *member* and says
   nothing about an added *variant*; `Part` travels on the wire inside `BundleReport::members`, and
   an older `mix` cannot decode one it does not have. `Part::Crashes` was free in T91 for the reason
   T89 could decline to repair two destructive migrations — nothing has ever been released from this
   repository. It will not be free next time. `MANIFEST_FORMAT` goes to 2 with it, so a reader that
   knows only the old archive stops with a number it can compare rather than a `serde` error.

6. **`mixengine-elevate` installs no hook**, and the reason is security rather than tidiness: it runs
   as root, and a root-owned file created inside a directory an ordinary account can write is a
   symlink target waiting for one. **`mix` and the shim install none either**: their stderr is a
   screen somebody is looking at, which is what a report is *for*.

## Consequences

**Easy:**

- A crash report can be attached to a public issue without being read first, which is the whole
  point and is a property of the file rather than of the person attaching it.
- Nothing about privacy has to be argued at install time, because nothing leaves.
- Three documents that described this behaviour become true, and `mix doctor` gains a line that says
  a bug happened here instead of the user discovering it by a service that quietly stopped being
  supervised.

**Hard, and accepted:**

- **A report that travels alone has no message.** The location is `file:line:col` in this
  repository, so for the `unwrap`/`expect` panics that are nearly all of them the line *is* the
  message; whoever wants more asks for the bundle, which carries the log.
- **Nobody here will ever know how often MixEngine crashes.** That is the same trade already made
  about Smart App Control's population, and it is made for the same reason.
- **The hook can deadlock**, and this is named rather than closed. A panic hook runs *before*
  unwinding, on the panicking thread, holding every lock that thread holds — so a panic raised inside
  the logging sink would have the hook's log line take a mutex the thread already owns. The write of
  the file is therefore ordered *first*, so the evidence survives a hang; `RotatingFile` returns its
  errors rather than panicking, which is what makes the case unlikely.
- **A `SIGKILL`, an OOM kill and a hardware fault leave nothing.** A panic hook sees panics. `mix
  doctor` says "crash reports" and never "crashes".
- **A panic before `logging::init` gets the default hook.** Covering it would mean a hook installed
  before the home is known, which is a hook with nowhere to write.

## Alternatives considered

**Upload to a crash service.** The literal reading of the word. It loses on three counts: it needs a
backend nobody here runs, it contradicts two accepted documents that promise no telemetry, and the
consent dialog it would require is worse for the user than the thing it is asking about — a file in
their own home.

**Record only after the user opts in.** Coherent, and the reading a careful reviewer reaches for. It
loses on when the switch is thrown: the first crash is the one nobody has consented to yet, and it is
the only one that exists at the moment somebody is trying to work out what went wrong. What is left
is a feature that works from the second occurrence of every bug.

**Keep the panic message and filter it.** `bundle_api.rs` already argues why not, about itself: *"a
filter layered on top would be a guess that a pattern matched — and worse than nothing, because it
would invite the next reader to believe the log is filtered rather than clean"*. A regular expression
over a `format!` string is exactly that guess.

**No file at all — only a log line.** The cheapest thing that makes `rust.md`'s sentence true, and it
was seriously considered. It loses because `daemon.log` carries the user's own paths, so the artifact
somebody can attach to a public issue would not exist — and producing one is the difference between
reporting a crash and merely surviving it.
