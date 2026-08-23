# T47a — `mix doctor`, which reports and does not repair

Roadmap task **T47a**, phase 4, split out of **T47**. Design settled 2026-08-24, before
implementation.

Everything under `.claude/` that this build touches is in force: no business logic in clients, no OS
calls outside `mixengine-platform`, cross-platform or not merged.

## Why T47 is split

T47 as the roadmap wrote it is four subsystems in one line: reconcile six kinds of state, flush the
privileged queue, state the per-OS orphan guarantee, detect Windows' excluded port ranges, re-check
the home's permissions, and decide whether `icacls` survives. A reviewer could accept any one of
those and reject its neighbour, which is the test for a task boundary.

The split is by **what the code does to the machine**, not by subsystem:

- **T47a** — `daemon.doctor` and `mix doctor`. Reads, reports, writes nothing.
- **T47b** — `daemon.doctor_repair`. Acts on what T47a found, and flushes the deferred privileged
  operations.

`daemon.doctor_repair` is already the name [client-surface.md](../../../.claude/features/client-surface.md)
promises a client, so this names the read half after the write half rather than inventing a pair.

## Scope

In: the report, its nine checks, the one new platform read they need, `mix doctor`, and the roadmap
split.

Out, each with an owner: repairing anything is **T47b**; **stale generated configuration is T47b's
too**, argued in D9; the diagnostics archive is **T93**; the complete uninstall path is **T87**.

---

## D1 — Two methods, one for reading and one for writing

`daemon.doctor` takes nothing and answers a `DoctorReport`. It is a read in the strict sense: no row
is written, no file is written, nothing is enqueued, and no elevation prompt can result from calling
it. That is what makes it safe to call from a client on a timer, from `mix status`, and from T93's
bundle.

## D2 — The report is a list of **checks**, not a list of problems

```rust
pub struct DoctorReport {
    /// Every check this build knows how to make, in a fixed order, whatever each answered.
    pub checks: Vec<Check>,
}

pub struct Check {
    /// What was examined, for a person: "the managed hosts block".
    pub name: String,

    /// What was found.
    pub outcome: Outcome,
}

pub enum Outcome {
    /// Examined, and what was expected is what is there.
    Ok,

    /// A fact worth stating that is not a fault — see D4.
    Note { because: String },

    /// Something is wrong. `id` is stable and is what T47b keys a repair off.
    Problem { id: ProblemId, because: String },

    /// Not examined, and why — an unsupported platform, or a subsystem switched off.
    Skipped { because: String },
}
```

**A doctor that prints nothing on a healthy machine leaves a person unsure it looked.** The list says
both what was examined and what was found, in one structure, so "nine checks, all Ok" is a rendering
of the same value as "nine checks, one Problem".

**`Skipped` is an outcome and not silence.** Windows' excluded port ranges do not exist on macOS or
Linux; the workspace rule is that an unsupported path returns a typed answer rather than `todo!()`,
and here that answer is a check that ran and says why it had nothing to examine.

## D3 — A `Problem` carries an id, never advice

`ProblemId` is a closed enum on the wire (`hosts_block_differs`, `resolver_not_wired`,
`home_permissions_lost`, …). The sentence in `because` is for a person; the id is what **T47b**
matches on.

This is T46's D4 held one task along, with one addition. T46 argued that a diagnostic must not
suggest a fix it cannot perform, because the advice drifts from the thing that performs it. An id is
not advice: it is a name for a condition, and it exists precisely so the two halves cannot drift —
a repair for an id nothing produces fails to compile against a closed enum.

## D4 — `Note` is a separate outcome from `Problem`, and the orphan guarantee is why

[ADR 0007](../../../.claude/decisions/0007-supervised-child-owns-a-process-group.md) settled that the
guarantee MixEngine can make about killing a service's descendants is **not the same on the three
systems**: total on Windows through a Job Object, the immediate child only on Linux, and none on
macOS. The ADR exists to stop Windows' promise being repeated where it is not true.

Reporting "macOS gives no guarantee" as a *problem* would be reporting the operating system as
broken, and a user cannot act on it. Reporting it as nothing at all would be the ADR's failure mode.
It is a `Note`: examined, stated, not a fault.

The same shape covers the helper's audit log, which lives outside `MIXENGINE_HOME` by design and is
removed only by T87.

## D5 — Nine checks

| # | Check | Built on |
| --- | --- | --- |
| 1 | the managed hosts block matches what this home's sites need | `core::hosts::desired` vs `hosts_file().managed()` — T41 |
| 2 | the resolver routes the managed TLDs here | `resolver().probe()` — T45 |
| 3 | the DNS server is answering, and which mode this home is in | `Dns::status()` — T44 |
| 4 | the front end may bind 80 and 443 | `PortAccess::probe` — T42 |
| 5 | nothing is waiting for permission | `elevation.status` — T40b, T64 |
| 6 | every declared domain, name by name | **renders `domain.dns_status`** — T46 |
| 7 | the home is still readable only by its owner | `is_restricted_to_owner` — T3a |
| 8 | what this system guarantees about a service's descendants | ADR 0007 — a `Note` |
| 9 | ports this system has reserved out from under us | **new** — D7 |

**Which outcome each check may produce**, so the vocabulary is decided here rather than nine times
during implementation:

| # | `Problem` id, when it is one | When it is a `Note` instead |
| --- | --- | --- |
| 1 | `hosts_block_differs` | — |
| 2 | `resolver_not_wired` | the DNS port is operating-system-chosen, so nothing may be wired to it (T45) |
| 3 | `dns_server_unavailable` — the bind failed | `hosts_only` for a stated reason, which is a supported mode and not a fault (T46a) |
| 4 | `port_access_missing` | — |
| 5 | `permission_pending` | — |
| 6 | `domain_unreachable`, once, naming the domains | every declared domain resolves |
| 7 | `home_permissions_lost` | — |
| 8 | never | always — this is the whole of D4 |
| 9 | `port_range_reserved`, only on an overlap | reservations exist and none overlaps |

Check 3's split is the one to get right. `hosts_only` is a **mode T46a closed as supported**, not a
degradation: a home whose wildcards are off and whose reason is stated is working as designed, and
calling it a problem would put a permanent fault on every machine that never wired a resolver.

Check 6 **calls the T46 report and does not recompute it**. T46's own roadmap entry says so in as
many words, and the reason is the one this whole design is built around: two implementations of one
question are two answers to it.

## D6 — Check 7 is `is_restricted_to_owner`'s first caller, and that settles the `icacls` question

T3a built `restrict_to_owner` and `is_restricted_to_owner`, and left a question open: on Windows the
check is narrow, because `icacls` prints localised account names and no SIDs, so the trustee list
cannot be verified — only whether inheritance was severed. Doing better means `GetNamedSecurityInfoW`
+ `GetSecurityDescriptorControl` for the `SE_DACL_PROTECTED` flag, and `GetAce` + `EqualSid` to
compare three trustees: roughly 150 lines of `unsafe` FFI.

T3a deferred it because the *apply* path was verified working and the check had no caller. **T47a is
the caller**, and the answer falls out of what this task needs rather than out of an argument: if the
whole of check 7 is "inheritance is intact, yes or no", `icacls` answers it and the FFI buys nothing.
That is what this task must state when it lands — and it is a decision reached by building the
caller, not by reading about the API.

## D7 — Windows' excluded port ranges, and why this check earns its keep

Windows reserves port ranges — Hyper-V, WSL, Docker Desktop and `winnat` all take them — and a bind
into a reserved range fails with an access error. **It looks exactly like a permission problem and is
not one**, so a person who hits it goes looking for elevation, UAC and firewall rules, none of which
are the answer. This is the single check in the list that saves a user from a wrong search rather
than telling them something they could have found.

Read with `netsh int ipv4 show excludedportrange protocol=tcp`, parsed in `mixengine-platform`. The
crate already spawns `lsof`, `icacls`, `resolvectl` and `pkexec` for reads of this kind, so this
follows a precedent rather than setting one. macOS and Linux answer `Skipped`.

It reports a `Problem` only when a range this home actually needs — 80, 443, or the DNS port — falls
inside a reserved one. A machine with reserved ranges that do not overlap is a `Note` at most: the
ranges are Windows' business until they collide with ours.

## D8 — `mix doctor` exits non-zero when it found a `Problem`

`Note` and `Skipped` do not. A `doctor` whose exit code says "I ran" rather than "the machine is
well" cannot be used in a script, and the shell is where the second question gets asked.

`--json` prints the report as it came. The human rendering is one line per check, and the sentence
under each that is not `Ok`.

## D9 — Stale generated configuration is T47b's

T47 named it, and it is deliberately not here. Deciding whether a file under `etc/` still matches the
state means **rendering the whole of it again and comparing** — which is precisely what the repair
path does before it installs anything. Building it in the read-only half means either building it
twice or building the repair early and calling it a diagnostic.

There is also a standing reason it matters less than it sounds: generated configuration is
disposable, regenerated from SQLite, and never parsed back. A file that has drifted is corrected by
the next write that touches it, so the fault it represents is "the front end is serving a stale
rendering **right now**", which is exactly the thing a repair fixes and a report cannot.

## D10 — Testing

Every check must be provable in both directions on all three systems, which the mock host already
makes possible for seven of the nine: `mock::Host` can present a hosts file that differs, a resolver
that routes nothing, a home whose permissions were lost.

Two are different and are honest about it. Check 9 has no mock worth writing — what it reads is one
machine's real reservations — so it is tested as *the parser* against captured `netsh` output, plus
one system-job assertion that the read itself does not fail on a real Windows runner. Check 8 is a
constant per system and is asserted as one.

**The trap this file inherits** is T45's D14 and T46's D9: a check that reports "nothing is wrong"
proves nothing unless the same test can make it report that something is. Every `Ok` assertion here
is paired with the arrangement that turns it into a `Problem`, in the same test.

## What this task does not settle

Whether `mix status` should print a one-line summary of the report. It is cheap and it is a
different question — `status` answers "what is running", `doctor` answers "what is wrong" — and
answering it here would mean deciding how often a client may call a read that spawns `netsh`.
