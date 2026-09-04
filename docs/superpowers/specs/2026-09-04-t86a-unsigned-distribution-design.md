# T86a — what an unsigned release does on the machines that judge it (design)

Roadmap task **T86a**, phase 9: *"Unsigned-distribution reality check for the **installer and the
updater**: SmartScreen behaviour across two consecutive releases; Gatekeeper flow on macOS 15+.
Document the findings in `updates.md`."*

The feature document is [updates.md](../../../.claude/features/updates.md), whose *Platform reality
when unsigned* section is what this task is here to replace with measurements. The elevation half of
the same question is [T41a](../../../.claude/roadmap/phase-4-sites-and-elevation.md); the certificate
half is [T94](../../../.claude/roadmap/phase-9-ship.md). All three are debts against **v0.1.0**.

## What this task actually is

Both readings the roadmap asks for are, as written, **dialogs a person sees**: "Windows protected
your PC", and macOS 15's System Settings → Privacy & Security → "Open Anyway". A dialog cannot be
asserted by CI, and that is why this task sat unrun beside T41a.

But under each dialog is a mechanism, and **each mechanism has an input a machine can read**:

- SmartScreen's Application Reputation gate is reached through `ShellExecute` on a file carrying
  **Mark-of-the-Web** — the `Zone.Identifier` alternate data stream with `ZoneId=3`, written by the
  application that downloaded it. No mark, no gate, whatever the file's reputation. So *"how often
  does a user see the warning"* reduces to **"which files in a MixEngine install ever carry a
  mark"**, and that is a property of our own artifacts.
- Gatekeeper's first-open gate is reached through **`com.apple.quarantine`**, likewise written by the
  application that downloaded the file. So *"is an update blocked"* reduces to **"does a file this
  product writes for itself carry quarantine"** — which is exactly the sentence
  [updates.md](../../../.claude/features/updates.md) already carries with *"Verify this empirically
  before relying on it"* against it.

So this task splits the question in two along a line the roadmap sentence did not draw: **the marks,
which are measured here on real artifacts on real Windows and real macOS; and the two dialogs, which
stay a person's and are handed a written procedure and a deadline.** That split is the whole design.

## Goal

A release engineer can say, from a CI log rather than from memory, exactly how many files an
unsigned MixEngine release ever puts in front of SmartScreen or Gatekeeper, and on which download
path. And the day Windows or macOS changes that, a job goes red naming the reading that changed
instead of a user discovering it.

## Scope

**In:**

- `packaging/windows/probe.sh` — the Windows readings, against the artifacts `windows/build.sh` just
  made: Authenticode status, Mark-of-the-Web propagation through the installer and through the
  portable zip, and whether a file written by an ordinary program carries a mark.
- `packaging/macos/probe.sh` — the macOS readings against the `.pkg`: signature status, Gatekeeper's
  own verdict with a control on whether Gatekeeper is answering at all, whether `installer(8)`
  installs a quarantined unsigned package, whether anything it installs carries quarantine, whether
  the ad-hoc signature is enough to execute, and the same written-file reading.
- Two steps in the `build` job of `.github/workflows/ci.yml`, one per OS, each printing its report
  into the step summary.
- The findings, in [updates.md](../../../.claude/features/updates.md) — replacing the reasoned
  paragraphs with dated, sourced measurements and keeping the reasoning that survives them.
- The procedure for the two readings a machine cannot take, in
  [build-and-release.md](../../../.claude/operations/build-and-release.md)'s release checklist, which
  is where a person is already standing in front of a draft release.
- `packaging/README.md` and the roadmap entry.

**Out:**

- **Turning any protection off to obtain a reading.** No `spctl --master-disable`, no Defender
  exclusion, no SmartScreen policy key. A measurement taken on a machine we disarmed is not a
  measurement of the machine a user has. Where a protection is already off, the probe says the
  reading is void — D9.
- **Smart App Control.** It is a different mechanism with a different answer, it is
  [T41a](../../../.claude/roadmap/phase-4-sites-and-elevation.md)'s and
  [T94](../../../.claude/roadmap/phase-9-ship.md)'s, and neither a GitHub runner nor this project's
  development machine has it enforcing — the developer machine's
  `VerifiedAndReputablePolicyState` reads `0` as of 2026-08-31. A probe cannot measure a policy that
  is not on, and pretending otherwise is the "green job that proves nothing" this repository keeps
  refusing.
- **Defender's `HostsFileHijack` heuristic.** T41a's second question, and it needs the elevated write
  rather than the artifact.
- **Replacing a running `mixengined.exe`.** A property of the update *sequence* and therefore
  [T88](../../../.claude/roadmap/phase-9-ship.md)'s, whose design already stops the daemon first.
  This task measures what the operating system does to a *distributed file*, not what an updater does
  to a running process.
- **Linux.** [updates.md](../../../.claude/features/updates.md) records "no obstacle", and there is
  no signature gate there to measure — `dpkg -i` and `rpm -i` on an unsigned package are
  unremarkable. The one real first-run friction on that platform, a browser dropping the AppImage's
  executable bit, is a browser's behaviour and not a signing question; it is documented in
  `updates.md` rather than built into a probe.
- **Buying anything.** T94's.

## Decisions

### D1 — A packaging script, not an `#[ignore]`d system test

Every other machine measurement in this product is a `cargo test` gated on `MIXENGINE_SYSTEM_TESTS=1`
in the `system` job ([testing.md](../../../.claude/standards/testing.md), rule 1). This one is not,
for one reason that decides it: **what is under test is an artifact, and artifacts exist only in the
`build` job.** The `system` job never runs `packaging/*/build.sh`, has no `.pkg` and no
`setup.exe`, and giving it one would mean a second release build on a third runner to feed a test
that then asserts nothing about our code.

The second reason is that nothing here is our code. `Get-AuthenticodeSignature`, `spctl`, `xattr` and
`installer` are the operating system's answers about a file. A Rust test binary would add a process
and a `Command` wrapper between the question and the answer, and would still shell out to the same
four tools.

So: one script per OS, beside the `build.sh` that made the thing it reads, in the language the rest
of `packaging/` is already written in (the T85 design, D9).

### D2 — The probe asserts the findings, so the document cannot rot silently

The readings are not printed and forgotten. Each one that is a statement about **our own artifacts**
is an assertion, and a run that contradicts it fails the `build` job. The installer that starts
propagating a mark, the `.pkg` that acquires a signature nobody bought, the binary that stops
executing without one — each is a change to the release story, and each should stop a release rather
than reach a user.

The cost is named and accepted: an operating system that changes its behaviour turns `build` red on
a commit that did not cause it. That is the alarm working. The report says which reading changed, and
the fix is to re-read `updates.md`, not to delete the assertion.

### D3 — Fail on our artifacts, record on the environment

Two kinds of thing can go wrong, and conflating them would make the gate untrustworthy:

| | Example | Probe does |
| --- | --- | --- |
| A statement about **our artifact** | the installer wrote a marked file | **fails** |
| A statement about **this machine** | `spctl` assessments are off; the runner is macOS 14; the shell namespace is unavailable | **records the reading as void, keeps going** |

A void reading is printed in its own section of the report, headed so that a reader of a green job
cannot mistake "not measured" for "measured and fine". This is the rule the T45 design paid for: four
of its six measurement rounds were void and nothing noticed until a control was added.

### D4 — Mark-of-the-Web is written by the probe, by hand, exactly as a browser writes it

There is no browser on a runner and no reason to want one. The mark is a two-line alternate data
stream, it is documented, and writing it is what a download manager does:

```
[ZoneTransfer]
ZoneId=3
```

The probe writes that stream onto a **copy** of the artifact and asserts it reads back before
concluding anything from it — a fixture with a control, for D3's reason. The same shape on macOS:
`xattr -w com.apple.quarantine "0081;<hex epoch>;Safari;<uuid>"`, the value Safari itself writes,
asserted back before use.

What this does not simulate is the **cloud verdict**: reputation is looked up per file hash and
signature, and no local fixture produces one. That is deliberate and it is the seam this design cuts
along — the mark is ours to measure, the verdict is the person's to observe (D10).

### D5 — The Windows readings

Against `target/packaging/dist/`, on the `windows-latest` leg.

- **W1 — nothing here is Authenticode signed.** `Get-AuthenticodeSignature` on `setup.exe` and on the
  three `.exe` files staged for it: every one `NotSigned`. Checked on PE files only — the `.zip` is
  not one, and asking about it would produce a `NotSigned` that means nothing.
  **This is the mechanism behind "every release resets it".** Reputation accrues to a *publisher*
  through a signature, or to a *file* through its hash. With no publisher identity there is only the
  hash, and the hash changes with every build. So the reset is not a thing to measure across two
  releases; it is a consequence of W1, and what two releases add is a person confirming it looked the
  way this predicts.
- **W2 — the fixture.** Write the `ZoneTransfer` stream onto a copy of `setup.exe`; read it back.
- **W3 — the installer does not pass its mark on.** Run the marked copy as
  `setup.exe /S /D=<temp dir>`; assert the three binaries arrive and that **none of them carries a
  `Zone.Identifier`**. Then uninstall and assert the directory is gone.
  The finding this produces is the one that matters most on Windows: **an install from a browser
  puts exactly one file in front of SmartScreen — the installer — and nothing it writes is ever
  judged again.**
- **W4 — the portable zip does pass its mark on, if Explorer opens it.** Mark a copy of the zip.
  Extract it twice: with `Expand-Archive`, and through the shell namespace Explorer itself uses.
  Assert no mark on the first; **expect** a mark on all three binaries from the second.
  If the shell namespace is unavailable on the runner, that half is a void reading (D3) and says so.
  The finding: the zip is the worse first-run path — three judged files instead of one — which is a
  documentation change and not a code one.
- **W5 — a mark is not a property of writing a file.** Write bytes to a new path with ordinary file
  I/O; assert no `Zone.Identifier`. **No network, deliberately**: the claim under test is not about
  one HTTP client, it is that Mark-of-the-Web is applied by an application that chooses to call the
  Attachment Manager, and a downloader that does not call it produces an unmarked file however it got
  the bytes. That is the reading the *updater* half of this task needs, and it holds for the
  `mix self-update` T88 has not written yet.
- **W6 — the installer's `PATH` edit is reversible.** Record `HKCU\Environment\Path` before, after
  the install and after the uninstall; assert the first and last are identical.
  Not asked for by the roadmap sentence and included anyway: this probe is the only thing in the
  repository that ever runs `AddToPath`, the function whose own comment says a truncation is
  "somebody's afternoon", and the assertion costs two lines.

### D6 — The macOS readings

Against the `.pkg`, on the `macos-latest` leg.

- **M0 — the controls.** Record `sw_vers` and `spctl --status` **first**. The roadmap asks about
  macOS 15+; a runner that is older answers a different question and the report says so. A machine
  with assessments disabled cannot give a Gatekeeper verdict at all, and M2 is void there.
- **M1 — the package carries no signature.** `pkgutil --check-signature` → no signature.
- **M2 — Gatekeeper's verdict, in its own words.** `spctl --assess --type install --verbose=4` on the
  package; record the exact text, assert a rejection. Void if M0 says assessments are off.
- **M3 — the fixture.** `com.apple.quarantine` onto a copy of the package; read it back.
- **M4 — does `installer(8)` install a quarantined, unsigned package?** `sudo installer -pkg <marked
  copy> -target /`, and record the answer either way. **This is the reading with the most product in
  it.** If it installs, then the macOS story for a command-line product is not "System Settings →
  Privacy & Security → Open Anyway"; it is one command, in the terminal the user already has open,
  and the documented instruction changes accordingly. If it refuses, the drop-off
  [updates.md](../../../.claude/features/updates.md) predicts is real and the recommendation to ship
  macOS only with a Developer ID gets its evidence.
- **M5 — nothing the package installs carries quarantine.** `xattr` on the three installed paths:
  empty. So the first run of `mix` after an install is not gated at all.
- **M6 — the ad-hoc signature is enough to execute.** `codesign -dv` on the universal `mix` records
  the signature it has; running it answers `--version`. This is the sentence in `updates.md` about
  Apple Silicon, measured rather than repeated.
- **M7 — quarantine is not a property of writing a file.** W5 on this platform, and the same
  argument: it is applied by a downloader that asks LaunchServices to apply it. This is the empirical
  verification `updates.md` asks for on the update path, and unlike the Windows half it can be
  strengthened later by T88's real downloader.

### D7 — The probe never installs without being asked, and never onto an occupied machine

M4 installs to `/`. There is no `-target` that isolates it, and the paths it writes —
`/usr/local/bin/mix`, `/usr/local/bin/mixengined`,
`/Library/PrivilegedHelperTools/dev.mixengine.elevate` — are the real ones. W3 is milder but still
writes `HKCU\Environment\Path`.

So the installing readings are gated on **`MIX_PROBE_INSTALL=1`**, set in the `build` job's step and
nowhere else. Without it the probe takes every non-installing reading and prints that the rest were
skipped — a partial run is honest, a surprise install is not.

**And the gate is not enough on its own.** With it set, the macOS probe still refuses when
`/usr/local/bin/mix`, the helper path, or a `dev.mixengine.cli` receipt already exists: on a runner
that is never true, and on a developer's machine it means the probe would overwrite a real install
and then delete it. It says which path is occupied and exits non-zero.

A variable of its own rather than `MIXENGINE_SYSTEM_TESTS`, because
[testing.md](../../../.claude/standards/testing.md) rule 1 says that one is set in exactly one place
— the `system` job — and that sentence is worth keeping true.

### D8 — Cleanup is a `trap`, and the Windows uninstall is synchronous or it is nothing

Both probes register their cleanup before the first thing that needs cleaning, so a failed assertion
leaves a runner and a developer machine as it found them.

The Windows half has a trap of its own that has to be named: **`uninstall.exe /S` returns
immediately.** NSIS copies the uninstaller into the temporary directory and re-executes it from
there, so the parent process exits while the deletion is still happening, and a probe that checks the
directory straight afterwards is asserting about a race. The documented answer is
`uninstall.exe /S _?=<install dir>`, which runs it in place and synchronously — and leaves
`uninstall.exe` itself behind, which the probe then removes. Both halves of that are in the script's
comments, because the second one looks like a bug.

### D9 — Never disarm the machine to get a reading

If `spctl --status` says assessments are disabled, the answer is a void reading, not
`spctl --master-disable`'s inverse. If SmartScreen is off by policy, the answer is a void reading,
not a policy key. Turning a protection off changes the machine into one no user has, and the number
that comes back is then about our tampering rather than about the product. It is worth writing down
as a decision because the temptation is strongest exactly when the job is otherwise about to report
nothing.

### D10 — What a machine cannot read goes in the release checklist, not in a document nobody re-opens

Two readings survive everything above:

1. **SmartScreen's actual verdict, on the first published release and again on the second.** It needs
   a real browser download of a real release asset, a cloud reputation lookup, and eyes. It is
   inherently a two-release reading and therefore **cannot be taken before v0.1.1 exists**.
2. **The macOS 15 dialog and the "Open Anyway" path**, in Finder rather than in `installer(8)`.

They attach to **release checklist item 4**, which already has a person in front of a draft release
smoke-testing each installer on a clean VM. The checklist gains what to do, what to record, and
where the record goes — `updates.md`, beside the measured half.

**And this resolves a contradiction the roadmap carries.** T86a's entry says v0.1.0 does not ship
before it is answered, while the SmartScreen half asks about *two consecutive* releases. Both cannot
be true. The split above is the honest reading: **the first-release dialog gates v0.1.0, the reset
across releases gates v0.1.1**, and the reset is not a surprise waiting to happen — W1 establishes
the mechanism that guarantees it. The roadmap entry is rewritten to say that rather than to leave a
reader to notice it.

### D11 — The report is not a release asset

The `release` job gathers `legs/*/*` into the distribution directory and signs **everything** it
finds. A probe report uploaded with the artifacts would therefore be signed, published, and listed on
the release page as though it were something to download.

So the report is written to `target/packaging/probe/<os>.md`, which is outside
`target/packaging/dist/` and outside the `upload-artifact` path, and it reaches a reader through
`$GITHUB_STEP_SUMMARY` and the job log. Nothing new is uploaded.

### D12 — One Windows leg, not two

`build` has two Windows legs since T85a. Mark-of-the-Web is not architecture-dependent, and the
arm64 leg would re-measure the same behaviour on a different file for the same conclusion at twice
the runner cost. The probe runs on `windows-latest` alone, and the report says which architecture it
read so nobody assumes both were checked.

## The report

One Markdown file per OS, and the same four sections in each, because a reader comparing two runs
should not have to re-learn the layout:

```
# Windows — unsigned distribution probe
Taken on <date> · <os build> · <runner image or machine> · x86_64
Artifacts: <name> <sha256 prefix> …

## Readings
| # | Reading | Result |
| W1 | setup.exe is Authenticode signed | no |
…

## Void readings
- W4 (shell extraction): the shell namespace was unavailable on this machine.

## What this does not answer
- SmartScreen's verdict …
```

The "What this does not answer" section is fixed text and is there for one reason: a green probe is
the most likely moment for somebody to believe the question is closed.

## Testing

The probes **are** the test, and there is no test of the probes — a second layer asserting that the
assertion script asserts would be ceremony. What keeps them honest instead:

- Every fixture is read back before it is concluded from (D4), so a probe that failed to write a mark
  cannot report that nothing propagated one.
- Every environment-shaped absence is a printed void reading (D3), so a run that measured nothing
  cannot look like a run that measured and found nothing.
- Both scripts run on every push to `master` and every dispatch, not only on a tag: `build` has no
  `if:` on it, so the readings are re-taken continuously and a change in OS behaviour is found on an
  ordinary day rather than during a release.
- There is **no `shellcheck` in `lint`** — checked, not assumed — so these two scripts are reviewed
  the way the five in `packaging/` already are, and they use the same `set -euo pipefail` preamble
  through `common.sh`.

## Risks

- **`installer(8)` may refuse.** Then M4 records a refusal, `updates.md` gets the harsher finding, and
  the recommendation to ship macOS only with a Developer ID is strengthened rather than contradicted.
  The probe asserts nothing about which way it goes — it asserts only that the reading was taken.
- **The macOS runner may be older than 15.** M0 records it; the reading then answers a nearby
  question and the report says which. `macos-latest` has tracked the current release closely enough
  that this is unlikely rather than impossible.
- **The shell namespace may be unavailable** on a runner that has no interactive session. W4's second
  half is void there, and the finding about the zip is then documented as the known Windows behaviour
  it is, without a local measurement behind it.
- **`build` can go red for something we did not change** (D2). Accepted, argued there.

## Files

| File | Change |
| --- | --- |
| `packaging/windows/probe.sh` | new — W1–W6 |
| `packaging/macos/probe.sh` | new — M0–M7 |
| `.github/workflows/ci.yml` | two steps in `build`, one per OS |
| `packaging/README.md` | what the probes are and how to run one by hand |
| `.claude/features/updates.md` | the findings, replacing the reasoned paragraphs |
| `.claude/operations/build-and-release.md` | the two manual readings, in checklist item 4 |
| `.claude/roadmap/phase-9-ship.md` | T86a's entry: what is measured, what remains, what it gates |
