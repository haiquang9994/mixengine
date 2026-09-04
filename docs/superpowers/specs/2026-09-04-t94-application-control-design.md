# T94 — What a certificate would repair, and what is left when it cannot (design)

Roadmap task **T94**, phase 9: *"Does a certificate this project can buy repair Smart App Control,
and what is left if it cannot?"* Split out of
[T41a](../../../.claude/roadmap/phase-4-sites-and-elevation.md) on 2026-08-24 and put here because
of what the answer changes: T41a's half can invalidate
[ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md) and five phases resting on it;
this half changes how the product is distributed, which is phase 9's business.

**The answer is no, and the interesting part is that it was never a question about money.** A
certificate signs the four binaries this project builds. Smart App Control judges every image *load*,
by file — so the binaries MixEngine exists to start are judged separately, and
[T20a and T27 measured](../../../.claude/operations/runtime-packaging.md) that of the four borrowed
runtimes only Node is signed upstream. A certificate therefore repairs the first image load and the
product dies at the second. That is the whole of Reading 1 and Reading 2, and it holds without buying
anything.

What is left is a product decision rather than a purchase, and it is
[ADR 0017](../../../.claude/decisions/0017-smart-app-control-is-an-unsupported-configuration.md):
**a machine with Smart App Control enforced is a configuration MixEngine does not support, names, and
refuses to work around.** The code in this task exists to make that sentence reach a person, because
today the same condition reaches them as `os error 4551`.

## Goal

Three readings, written down where the two beside them already are
([updates.md](../../../.claude/features/updates.md)), and one behavioural change: on Windows,
MixEngine can say the word *Smart App Control* rather than a number.

Concretely, after this task:

- `mix doctor` on Windows carries a check for this machine's Application Control policy — `Ok` when
  nothing is enforcing, a `Note` while Smart App Control is evaluating, and a `Problem` that
  `doctor_repair` explicitly declines to repair when it is enforced.
- A runtime whose post-install smoke test is refused at image load, and a supervised service whose
  spawn is refused, both say so in those words instead of carrying an OS error number to a person who
  has no reason to recognise it.
- `.claude/features/updates.md`, `runtime-packaging.md`, `testing.md`, `security-model.md`, T41a and
  T20a all point at one answer instead of at an open question.

## Scope

**In:**

- `mixengine-platform`: `traits/app_control.rs` — the `AppControl` capability, `AppControlState`,
  the pure `AppControlState::from_policy_value`, the `refused_by_app_control` classifier and the
  `APP_CONTROL_REFUSAL` sentence; `windows/app_control.rs` (the registry read), `linux/app_control.rs`
  and `macos/app_control.rs` (`UnsupportedPlatform`), `mock/app_control.rs`; one accessor on `Host`.
- `mixengine-proto`: one variant, `ProblemId::ApplicationControlEnforced`.
- `mixengine-daemon`: one check in `doctor.rs`, one arm in `repair.rs`.
- `mixengine-core`: `install::smoke` names the refusal in the `detail` it already carries.
- `mixengine-supervisor`: one `Error` variant for a spawn the policy refused.
- Docs: the findings section in `updates.md`, ADR 0017, and the cross-references that currently say
  the question is open.

**Out:**

- **Buying a certificate.** Reading 2 settles the question without one: a certificate that worked
  perfectly on our four binaries would leave `php.exe`, `nginx.exe` and `caddy.exe` exactly where
  they are. Spending a few hundred dollars to confirm a conclusion that does not depend on the answer
  is not a measurement.
- **A new reading in `packaging/windows/probe.sh`.** T86a's **W1** already measures that `setup.exe`
  and all three binaries report `NotSigned`, which *is* Reading 1. Re-measuring the upstream runtimes
  would mean downloading PHP, nginx and MariaDB inside the `build` job to confirm what T20a wrote
  down, at real CI cost and no new information.
- **Counting the population.** Argued in D8: the two remedies it was supposed to choose between are
  refused at every population size, so the number decides nothing and nobody here can measure it.
- **Making `mixengined` survive its own refusal.** On a machine where the daemon's image is refused,
  nothing of ours runs and the only record is Windows' own `Microsoft-Windows-CodeIntegrity/Operational`
  log. That is stated in D2 rather than pretended away.

## The readings

### Reading 1 — what a certificate covers

Four images, and they are the whole of what an Authenticode certificate this project bought would
sign:

| Image | Where it comes from |
| --- | --- |
| `mix.exe` | this workspace |
| `mixengined.exe` | this workspace |
| `mixengine-elevate.exe` | this workspace |
| `mixengine-shim.exe` | this workspace, copied into `<root>/bin` once per command name |

T86a's **W1** measured all of them, plus `setup.exe`, as `NotSigned` today.

### Reading 2 — what it leaves uncovered

Every other image a working MixEngine install loads. Measured by T20a and T27 and recorded in
[runtime-packaging.md](../../../.claude/operations/runtime-packaging.md):

| Image | Authenticode |
| --- | --- |
| `php.exe`, `php-cgi.exe`, `php-win.exe`, `phpdbg.exe` and the DLLs beside them | **NotSigned** |
| `nginx.exe` | **NotSigned** |
| `caddy.exe` | **NotSigned** |
| `python.exe`, `python3xx.dll` | **NotSigned** |
| `ruby.exe`, `x64-ucrt-rubyXXX.dll` | **NotSigned** |
| `node.exe` | Valid — `CN=OpenJS Foundation`, and the only one |

Add the service binaries a MixEngine install starts — `mariadbd.exe`, `postgres.exe`,
`memcached.exe`, a Redis-compatible server — and the shape of the answer does not change.

**Smart App Control judges an image load, not a process tree.** A signed `mixengined.exe` that spawns
an unsigned `caddy.exe` does not lend it anything: the second load is judged on its own file. So the
covered set is four images and the uncovered set is everything the product is *for*.

That is the finding, and it is the reason this task did not need to buy anything to answer its own
question.

### Reading 3 — the cheapest thing that covers the rest

The roadmap names three candidates. Two are refused on their own merits, at any population size:

| Candidate | What it actually costs | Verdict |
| --- | --- | --- |
| Rebuild and sign the runtimes | A build pipeline for PHP and its extensions, nginx, MariaDB, PostgreSQL, Redis, Ruby and Python, on two Windows architectures, maintained for as long as the product exists — including security updates for seven upstreams | **Refused.** This is precisely the maintenance cost *"borrow before you build"* declined, and signing does not reduce it by a line |
| Ask the user to turn Smart App Control off | A one-way door on their machine: SAC cannot be re-enabled without reinstalling Windows | **Refused.** A development tool has no business asking somebody to permanently lower their machine's defences so that it can run |
| Accept the loss and name what it costs | Nothing to build, and one condition to report honestly | **Chosen** |

**A fourth candidate exists and is worth writing down so nobody proposes it as new.** Smart App
Control admits a file on ISG reputation as well as on signature, and reputation accrues to a
*publisher* when there is one and to a *file hash* when there is not — which is the same mechanism
behind W1's "every release resets whatever reputation the last one earned". So a certificate would
genuinely improve MixEngine's own four binaries over time. It buys nothing at all for the borrowed
ones, and it is the borrowed ones that make the product a product. There is no supplemental policy to
author either: SAC's base policy is Microsoft-signed and does not accept one.

## The types

```rust
/// What this machine's Application Control policy is doing about unsigned images.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppControlState {
    /// Nothing is refusing an image for want of a signature.
    Off,

    /// Smart App Control is evaluating this machine and has not decided yet.
    Evaluation,

    /// Smart App Control is enforcing. Unsigned images without reputation are refused at load.
    Enforced,

    /// The policy value is one this build has no name for.
    Unknown {
        /// What was actually read, so a bug report carries it.
        value: u32,
    },
}

/// What this machine's Application Control policy is doing — roadmap task **T94**.
pub trait AppControl: std::fmt::Debug + Send + Sync {
    /// This machine's answer.
    ///
    /// # Errors
    ///
    /// `Error::UnsupportedPlatform` on a system with no such mechanism, which `mix doctor` renders
    /// as a check that ran and says why it had nothing to examine; `Error::Os` when the policy could
    /// not be read.
    fn state(&self) -> Result<AppControlState>;
}
```

and two free items beside them:

```rust
/// Was this image load refused by an application control policy?
pub fn refused_by_app_control(error: &std::io::Error) -> bool;

/// Why an image load was refused, in one sentence, for whoever reads the failure.
pub const APP_CONTROL_REFUSAL: &str = "…";
```

## Decisions

### D1 — The registry value, not `Get-MpComputerStatus`

Smart App Control's state is readable two ways: `Get-MpComputerStatus`'s `SmartAppControlState`
property, and `HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy\VerifiedAndReputablePolicyState`.

The registry value wins for two reasons. `platform-abstraction.md` rule 5 asks for the API where
there is one, and the alternative here is not an API — it is spawning PowerShell and parsing a
localised object. And the registry value is the one with **evidence at both ends of its range**: it
read `1` on a developer machine with SAC enforcing on 2026-08-13 (the reading `updates.md` records),
and `0` on the same machine on 2026-09-04 after SAC was turned off, with `SAC_PreviousState = 1`
beside it. A reader validated against both of its own readings is worth more than one validated
against a document.

`windows-sys` already carries `Win32_System_Registry` — for `windows/path.rs` — so this adds no
dependency and does not move `mixengine-elevate`'s dependency closure.

### D2 — This check is about the machine MixEngine *can* run on, and says nothing about the one it cannot

The obvious objection: a check that only runs when `mixengined.exe` loaded cannot report the machine
where `mixengined.exe` did not.

That is true and it does not make the check empty, because **the judgement is per file**. The
evidence in `updates.md` is that a refusal on a first-seen file did not persist — the same binaries
ran unchanged hours later — so a machine can be enforcing and still be running this daemon while a
`php.exe` that was downloaded five minutes ago is refused. The reachable middle is real, and it is
exactly the ground `mix install` stands on.

On the machine where our own image is refused, there is no MixEngine to ask. The only record is
Windows' `Microsoft-Windows-CodeIntegrity/Operational` log, events 3033/3077/3118. That is written
into `updates.md` as the diagnosis of last resort rather than left for somebody to discover.

### D3 — Two claims, kept apart: Smart App Control, and an application control policy

`VerifiedAndReputablePolicyState` describes **Smart App Control** and nothing else. An enterprise WDAC
policy refuses image loads through the same Code Integrity subsystem, with the same
`os error 4551`, while that value reads `0`.

So the two halves of this task make two different claims, deliberately:

- The doctor check names **Smart App Control**, because that is what it read.
- `refused_by_app_control` and `APP_CONTROL_REFUSAL` say **"an application control policy on this
  machine"**, because that is all a `4551` proves.

Collapsing them would put a sentence about Smart App Control in front of somebody on a corporate
laptop whose Smart App Control is off — and send them to a setting that is not the one refusing them.

### D4 — `4551` is a documented lower bound, not a Win32 symbol

The constant is declared with its provenance — the message Windows produced when the refusal was
measured, *"An Application Control policy has blocked this file"* — rather than by guessing at a
symbol name `windows-sys` may not export.

And it is a **lower bound**: it is the only code this project has observed for this condition, not a
proof that it is the only one. The classifier answers `false` for everything else, so the failure mode
is a diagnosis that does not appear, never one that appears wrongly. If another code turns up, it
joins the constant beside a reading, the way this one did.

`refused_by_app_control` is written as `cfg!(windows) && error.raw_os_error() == Some(…)` rather than
behind `#[cfg(windows)]`, so the function compiles on all three systems and its tests run on all
three — including the one asserting it is `false` off Windows. This is the rule `reserved::parse` and
`resolver::directory` already follow: the half that is a decision is tested everywhere, and only the
call that can be made nowhere else hides behind a `cfg`.

### D5 — A key that is not there is `Off`, not `Skipped`

Smart App Control exists on Windows 11 22H2 and later. On Windows 10, on Server, and on a machine
that has never had it, the value — or the whole `CI\Policy` key — is simply absent.

`Skipped` would be a lie in the other direction: it means *nobody looked*, and somebody did. The
question this check asks is "is anything refusing images here for want of a signature", and no key is
a clear **no**. So absent key or absent value reads as `Off`, and the reader says so in its own
documentation rather than leaving a reader to infer it.

A value that is present and is not 0, 1 or 2 reads as `Unknown { value }` and the check reports
`Skipped` carrying the number — because a build that has no name for a state must not guess which of
the named ones it resembles.

### D6 — Enforced is a `Problem`, and `doctor_repair` declines it out loud

`Outcome::Note` is for a fact nobody can act on, and `Outcome::Problem` for something that is wrong.
This is the second, on `PortRangeReserved`'s precedent: the system did it, MixEngine cannot undo it,
and it still breaks the product.

The cost is stated rather than hidden: on an enforcing machine where everything currently runs,
`mix doctor` will report a problem and exit non-zero. That is the right answer anyway — the *next*
image load is a runtime archive whose hash has never existed on any machine in the world, which is
precisely the first-seen case the evidence says is refused. A doctor that called that machine well
would be committing the failure `Outcome::Skipped` exists to prevent.

`repair::plan_for` gains `Planned::Untouched` for it, beside `PortRangeReserved` and
`DnsServerUnavailable`. Its reason is the second half of ADR 0017 in one sentence: turning Smart App
Control off is a one-way door on the user's own machine, and MixEngine will not ask for it.

Evaluation mode is a `Note`: the machine has not decided, and there is nothing to do about a decision
that has not been made.

### D7 — Two callers name the refusal, and neither needs a new type

The two places MixEngine loads an image it did not build are `install::smoke` — the post-install check
that runs a freshly downloaded runtime once — and the supervisor's spawn.

`install::smoke` already funnels every failure through a `detail: String`, so the sentence is appended
there.

**This decision asked for a new `Error` variant in `mixengine-supervisor` and it was wrong to.**
Planning found that `mixengine_supervisor::Error::Spawn` is already *classified* one crate over: the
daemon's `ToWire` matches it and picks a hint by `source.kind()`, which is where a missing program
becomes `DependencyMissing` and a missing executable bit becomes advice. A refusal is a third answer
to that same question, so it is a guard arm above that match — before `kind()`, because a Code
Integrity refusal has no distinctive `ErrorKind` to key off. The supervisor crate is untouched, and
the classification stays in the one place that already owned it.

The *answer* has one definition (`refused_by_app_control`) and the *sentence* has one definition
(`APP_CONTROL_REFUSAL`); what differs between the two sites is only where it is attached.

**The smoke test is the load that matters most**, and that is why it is one of the two. A runtime
archive that finished downloading thirty seconds ago is the purest first-seen file this product ever
produces.

### D8 — The population is not counted, and the decision does not need it

The roadmap says the population is worth counting for exactly one of the three remedies. Reading 3
dissolves that precondition: the other two are refused at *every* size — one because it re-argues a
maintenance decision that has only got more expensive, the other because it asks a user to disable a
security feature permanently. When 1% and 90% lead to the same move, the number is not a measurement,
it is a delay.

And nobody here can take it. There is no telemetry, and T91's crash reporting is specified as opt-in
and free of project paths and credentials — it is not an inventory of machines and was never meant to
become one. What is written down instead is the *mechanism* — SAC ships
enabled on clean Windows 11 installs, stays off after an in-place upgrade, and takes itself out of
evaluation mode when it observes development activity, which is a description of MixEngine's own
audience — labelled as reasoning rather than as a reading, because that is what it is.

**What would reopen this** is named in ADR 0017 so a future reader does not have to reconstruct it:
a Windows in which SAC accepts a publisher allow-list, or an upstream supply in which the runtimes
this product borrows arrive signed.

### D9 — ADR 0005 is confirmed, not superseded

The roadmap wrote that a bad answer here *supersedes*
[ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md), because "no OS code signing"
would have stopped being a trade of first-launch friendliness against a few hundred dollars a year.

It has not stopped being that trade, and the reason is Reading 2. ADR 0005 declined to buy
certificates for MixEngine's own binaries; buying them would not have produced a product that runs
under Smart App Control, because the binaries that decide the outcome are not ours to sign. The
certificate was never the thing standing between this product and SAC.

So ADR 0017 is a new decision beside 0005 rather than a replacement of it: *what happens on a machine
that enforces*, which 0005 never addressed.

### D10 — Not cached

One registry open, one query, one close per `daemon.doctor` call. `mix status` and T93's bundle both
reach it, and it is microseconds.

Caching it would be actively wrong: Smart App Control can take itself out of evaluation mode while
the daemon is running, and a cached value would be stale at the exact moment it started to matter.

## Data flow

```
mix doctor
  └─ daemon.doctor
       └─ Doctor::application_control()
            └─ Host::app_control().state()
                 ├─ windows  → RegQueryValueExW(HKLM\…\CI\Policy, VerifiedAndReputablePolicyState)
                 │              └─ AppControlState::from_policy_value(Option<u32>)
                 ├─ linux    → Err(UnsupportedPlatform)   → Outcome::Skipped
                 └─ macos    → Err(UnsupportedPlatform)   → Outcome::Skipped

install::smoke ─┐
                ├─ refused_by_app_control(&io::Error) ─→ + APP_CONTROL_REFUSAL
supervisor spawn┘
```

## Testing

Unit tests beside each piece, and all of the pure ones run on all three systems:

- `AppControlState::from_policy_value`: `None` → `Off`; `0` → `Off`; `1` → `Enforced`; `2` →
  `Evaluation`; anything else → `Unknown { value }` carrying the number.
- `refused_by_app_control`: the measured code → `true` on Windows and `false` elsewhere; an ordinary
  `NotFound` → `false` on all three; an error with no OS code → `false`.
- The Windows reader, `#[cfg(windows)]`: it answers without an error on the machine running the
  suite, whatever that machine's state is. An assertion on the *value* would be an assertion about
  the runner rather than about the code.
- The doctor check: through `mock::Host`, one test per state — `Off` → `Ok`, `Evaluation` → `Note`,
  `Enforced` → `Problem { ApplicationControlEnforced }`, `Unknown` → `Skipped` carrying the number,
  `UnsupportedPlatform` → `Skipped`.
- `repair::plan_for`: the existing table test gains the new id and asserts it is `Untouched`. It
  would not compile without an arm, which is that table's whole design.

## Risks, and where each is answered

| Risk | Answer |
| --- | --- |
| A new `ProblemId` variant breaks an older `mix` that cannot deserialise it | Real, and it is the class **T88c** is open for. Not solved here: this is a `ProblemId` behaving like every other `ProblemId`, and fixing one half of that rule buys nothing while the other half is unfixed |
| The check names Smart App Control on a machine refused by an enterprise WDAC policy | D3 — the two claims are kept apart on purpose |
| `4551` is not the only code | D4 — it is a lower bound, and the classifier's failure mode is silence rather than a wrong diagnosis |
| The doctor reports a problem on an enforcing machine that currently works | D6 — accepted, and argued: the next image load is the first-seen case |
| Reading `HKLM` needs privilege | It does not: `HKLM\SYSTEM\CurrentControlSet\Control` is readable by any account, and the call is a query with no write anywhere near it. `daemon.doctor`'s "nothing here writes" is intact |
| The state reaches T93's diagnostics bundle | It does, and it is worth saying rather than discovering: it is one machine-wide security setting with no identifier attached to it, in a bundle the user chooses to share. Named here so the review of that bundle's contents has it |
| `mixengine-elevate`'s dependency closure moves | It cannot: no crate is added, and `Win32_System_Registry` is already enabled for `windows/path.rs`. CI's diff against `.github/elevate-dependencies.txt` is the check |

## What building it changed

**The supervisor needed no new error variant** — D7 above, rewritten rather than left standing. The
design reasoned from the enum being `#[non_exhaustive]`; what it had not read is that the daemon's
`ToWire` already classifies a `Spawn` by its source to choose a hint, so the refusal is a third arm
of an existing decision instead of a fourth shape of error.

**The workspace's own layering test caught the first attempt at the tests, and it was right to.**
`workspace_layering::no_crate_but_platform_compiles_a_line_away_by_operating_system` refuses a
`#[cfg(windows)]` in `mixengine-core` or `mixengine-daemon` — including in a test module — and both
new tests had one, because the expected answer differs by system. The fix is the one that test names
in its own failure message: assert against `cfg!(windows)` as a *value*, in one test with both arms.
That is better than what was written: the classifier's `cfg!(windows)` is the thing under test, and
asserting it from the caller's side is what stops the two drifting.

**`rustfmt` reorders `mod` declarations and leaves their comments behind.** Inserting
`mod app_control;` after `mod access;` in `windows/mod.rs` moved the item into alphabetical position
and stranded its comment on the module above it. Worth recording because the damage is silent and
`cargo fmt --check` is happy with the result: put a new `mod` line in its *sorted* position to begin
with, and the comment stays with it.

**The doctor's tests are over free functions, not over a `Doctor`.** `limit_outcome` and
`foreign_rule_outcome` are already free and pure so that every arm can be driven from a value written
by hand; `app_control_outcome` follows them rather than inventing a harness that builds a whole
report over a mock host.

**Two report-length assertions had to move**, and they are the only reason a check cannot be added
silently: `crates/mixengine-daemon/tests/api.rs` and `crates/mixengine-cli/tests/doctor.rs` both count
the checks. The CLI test gained an assertion that the new one reaches the screen, on T76's precedent.

**The second validating reading was taken during this task.** The design cites
`VerifiedAndReputablePolicyState = 1` from 2026-08-13; `0` was read on the same machine on
2026-09-04, with `SAC_PreviousState = 1` and `SAC_EnforcementReason = 6` beside it. Both ends of the
reader's range are now evidence rather than one end and a document.

## What this leaves

- **T41a's own half is untouched.** Whether the elevated hosts write survives Defender's
  `HostsFileHijack` heuristic still needs a clean Windows VM and a person. What closes here is the
  remedy half that moved out of it on 2026-08-24.
- **The population.** D8 — not measured, and the decision does not rest on it.
- **The two dialogs.** T86a's release-checklist item 4 is unaffected: SmartScreen's verdict on a
  browser download and macOS 15's "Open Anyway" flow are a different mechanism with a human in it.
- **A second code for a refused image load**, if one ever appears. D4 says how it joins.
