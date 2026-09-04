# T87 — Complete uninstall, and proving nothing is left behind (design)

Roadmap task **T87**, phase 9: *"Complete uninstall path + a clean-VM smoke test proving nothing is
left behind."* `--dry-run` is this task's, moved here from M4 on 2026-08-24 — a milestone three
phases earlier cannot require a run of something that does not exist yet, and a dry run belongs
beside the thing it is a run of.

What has to come off is written down in three places already, and this task is the first thing that
reads all three at once:
[overview.md](../../../.claude/architecture/overview.md)'s *"Nothing is written outside this root
except…"*, [ADR 0015](../../../.claude/decisions/0015-the-helper-installs-itself.md) (*"Uninstall
gains a second root-owned file outside `MIXENGINE_HOME` to remove, beside the audit log"*), and
[ADR 0016](../../../.claude/decisions/0016-autostart-is-registered-by-mixengine.md) (*"Uninstall
gains one more thing to remove, and `autostart.disable` to remove it with"*).

Two things this task changes about the sentence it was written from, both argued below: it is
**two methods and not one flag** (D2), on `daemon.doctor`/`daemon.doctor_repair`'s split; and
**Windows cannot delete the helper it is running from** (D8), so on that system one file leaves at
the next restart and the report says so rather than claiming a removal that did not happen.

## Goal

A person who has used MixEngine, changed their mind, and typed one command finds: no MixEngine block
in the hosts file, nothing routing `.test` anywhere, no capability or packet-filter redirect, no
boot-time job, no certificate authority in any store their browsers or their operating system read,
no firewall rule, no autostart entry, no `PATH` entry, no privileged helper, no root-owned audit log,
and no `MIXENGINE_HOME`. And before they type it, `mix uninstall --dry-run` tells them each of those
things by name and by location, changes nothing, and needs no administrator.

The smoke test that proves it is CI's `system` job on a fresh runner — which is the clean VM the task
asks for — reading the machine back with the operating system's own tools rather than with
MixEngine's.

## Scope

**In:**

- `mixengine-proto`: `uninstall_api.rs` — `UninstallQuery`, `UninstallReport`, `Residue`,
  `ResidueId`, `Disposition`; two method names on `rpc::method`.
- `mixengine-proto/privileged.rs`: two operations, `HelperRemove` and `AuditLogRemove`, each
  carrying nothing.
- `mixengine-elevate`: `helper::remove`, `audit::remove`, their gates, and the one change to
  `process()` that lets a log not record its own deletion.
- `mixengine-platform`: `install::remove_helper`, `elevated::remove_root_owned_file` and
  `elevated::remove_root_owned_directory`, per-OS.
- `mixengine-daemon`: `uninstall/` — `inventory.rs` (the one enumeration) and `mod.rs` (the act),
  plus two RPC methods and the arming of the home's removal on the way out.
- `mixengine-cli`: `mix uninstall`, its confirmation, its rendering, and the verification it does
  once the daemon is gone.
- Tests: unit tests beside each new function; `crates/mixengine-cli/tests/uninstall.rs` with an
  unignored half (the plan, the refusals, `--keep-home`) and an `#[ignore]`d half (the round trip);
  one CI step per system printing what the suite left.

**Out:**

- Removing the *program* — `mixengined`, `mix`, `mixengine-shim`. Those arrived through an installer
  or a zip and leave the way they came (`build-and-release.md`); an uninstall that deleted the
  binary it is running from would be a different task on three packaging formats.
- Un-registering projects from the user's own repositories. A `mixengine.toml` in somebody's
  checkout is theirs, and it is not something MixEngine wrote outside its root.
- Anything about updates. T88's `mix self-update` is the neighbouring task and shares no code here.

## The types

```rust
/// `daemon.uninstall_plan` and `daemon.uninstall` take the same query.
pub struct UninstallQuery {
    /// Leave `MIXENGINE_HOME` where it is, and undo only what is outside it.
    #[serde(default)]
    pub keep_home: bool,

    /// Flush the elevation queue in this same call, raising the one prompt.
    /// Ignored by `daemon.uninstall_plan`, which raises nothing.
    #[serde(default)]
    pub grant: bool,
}

pub struct UninstallReport {
    /// One entry per thing MixEngine can have written, in a fixed order, whatever each answered.
    ///
    /// Ten of the eleven ids appear exactly once. `RelocatedDirectory` appears once per directory
    /// `[paths]` has moved out of the root, and on an ordinary home not at all — see D9.
    pub items: Vec<Residue>,

    /// The single grant this call raised, when it was asked to and anything needed the helper.
    pub granting: Option<JobSummary>,
}

pub struct Residue {
    pub id: ResidueId,
    /// What it is, for a person: "the managed hosts block".
    pub what: String,
    /// Where it is, for a person: "/etc/hosts", "HKEY_CURRENT_USER\\Environment\\Path".
    pub location: String,
    pub outcome: Disposition,
}

#[serde(rename_all = "snake_case")]
pub enum ResidueId {
    HostsBlock, ResolverWiring, PortAccess, FirewallRules, TrustStore, BrowserTrust,
    PrivilegedHelper, AuditLog, AutostartEntry, PathEntry, Home, RelocatedDirectory,
}

#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum Disposition {
    /// Nothing of ours is there. The ordinary answer on most of the list, on most machines.
    Absent {},
    /// What would be done. `daemon.uninstall_plan` answers only this, `Absent` and `Kept`.
    Planned { how: String },
    /// Gone, measured after the fact rather than claimed.
    Removed { what: String },
    /// Waiting on a prompt nobody has answered yet — `grant: false`.
    Enqueued { what: String },
    /// Goes when this daemon exits, which it is about to do.
    OnExit { what: String },
    /// Goes at the next restart of this machine, and why it could not go now.
    OnRestart { what: String },
    /// Deliberately left — `keep_home`.
    Kept { because: String },
    /// Tried, and it is still there.
    Failed { because: String },
}
```

`UninstallReport::left_behind()` is `any(Failed)` and is `mix uninstall`'s exit code. `OnExit` and
`OnRestart` are not failures: one is a removal this process is performing and the other is one the
operating system has accepted.

## Decisions

### D1 — One inventory, and it is the readers `mix doctor` composes rather than its report

The roadmap sentence is *"T47's `mix doctor` already enumerates most of that to reconcile it; this
reads the same inventory rather than building a second one."* What is shared is the **readers** —
`Host::hosts_file().managed()`, `resolver().probe()`, `trust_store().probe()`,
`port_access().probe()`, `firewall_rules()`, `path_integration()`, `service_installer()`,
`browsers()`, and `mixengine_core::certs::ca::read` — every one of which already has exactly one
implementation, in `mixengine-platform`, and is already what the doctor asks.

What is **not** shared is `DoctorReport`, and that is a decision rather than an oversight. That
document answers *"is this as it should be?"*, and this one asks *"is any of ours there?"* — the two
are not the same question and `Outcome::Ok` does not mean the same thing across the report's own
rows. `Ok` on the trust check means the authority **is** installed; `Ok` on the hosts check means the
block **matches** what the sites need, which on a wired machine is an *empty* block. An uninstall
driven off that would remove the trust store and skip the hosts block on one machine and do the
reverse on the next. So the inventory is taken from the probes and never from the judgement.

The one enumeration that does exist is `uninstall::inventory::take`, and **both** methods call it:
the plan renders it, the act renders it, acts, and then calls it again to measure. Two enumerations
— one for the dry run and one for the real run — is the "second inventory" this decision refuses,
and it is the one that would actually have been built.

### D2 — Two methods, not one flag: `daemon.uninstall_plan` and `daemon.uninstall`

`--dry-run` reads as a flag on one method. It is built as two, on the split this daemon already has
between `daemon.doctor` and `daemon.doctor_repair`:

- `daemon.uninstall_plan` is a **read in the strict sense** — no row written, nothing enqueued, no
  prompt possible, safe from a script and from a client that is only showing somebody what would
  happen. Exactly what makes `daemon.doctor` safe on a timer.
- `daemon.uninstall` acts, and answers a **`JobSummary`**, because it can raise the elevation prompt
  and a prompt is a person reading a dialog with no deadline — `cert.ca_uninstall`'s shape, and for
  its reason.

A single method returning either a plan or a job depending on a boolean is a method with two answer
shapes, which is a thing every client has to branch on and a thing `ts-rs` (T56) cannot describe
honestly. The cost is one more name; the gain is that the read half is provably a read.

### D3 — The act measures, it does not claim

`cert.ca_uninstall` established this and it is copied wholesale: after the grant, every privileged
item is **probed again** and its `Disposition` set from what the machine now says. The helper is
honest about what it did, but it is a separate process describing finished work; a fresh read costs
no privilege on any of the three systems for any item on this list, including the two root-owned
files — the audit log directory is world-readable by construction (`create_root_owned_directory`),
and `helper_path()` is a `stat` in a directory anybody may list.

So `Removed` is a measurement, `Failed` carries the reason the helper gave *and* the fact that the
thing is still there, and a grant the user declined produces `Failed` on every privileged row rather
than a single unrelated error.

**The second reading happens only when a grant was flushed.** With `grant: false` nothing has been
applied yet and the privileged rows stay `Enqueued` — re-probing there would find every one of them
still present and report `Failed` for work nobody has been asked to allow, which is the opposite of
what the two-call path is for.

The one item this cannot hold for is `Home`, because a process cannot measure the removal of the
directory it is running out of. That is `OnExit`, and D9 is who measures it.

### D4 — Two new privileged operations, and each carries nothing

`HelperRemove {}` and `AuditLogRemove {}` — no fields, on `HelperInstall`'s rule (the T85 design,
D2): the path is a constant compiled into `mixengine-elevate`, so neither operation hands a
compromised daemon a *delete this file as root* primitive. A `path` field here would be `Exec { cmd }`
with two more steps, which the closed-enum rule in
[security-model.md](../../../.claude/architecture/security-model.md) exists to refuse.

Dedupe keys: `HelperInstall` and `HelperRemove` share `"helper"` — two values of one question, *is
the helper where it belongs?* — so a removal enqueued behind a pending install replaces it, which is
`PortAccess`'s and `TrustStore`'s arrangement. `AuditLogRemove` is its own key, `"audit-log"`, and
has no opposite: nothing installs the log, the helper creates it on first run.

Both `requires_elevation()`. Both are removals inside directories only an administrator can write, so
there is nothing either could do under an ordinary token but fail.

The rest of the batch needs no new operation, which is the point of having built the reversals with
their mechanisms in view: `HostsApply { entries: [] }` clears the block (whole state),
`ResolverRevoke`, `PortAccessRevoke` and `TrustCaRemove` were shipped by T45, T42 and T49a
respectively for exactly this caller, and `FirewallApply { ports: [] }` is the firewall's revoke.

### D5 — A log cannot record its own deletion, so it is applied last and recorded nowhere

`mixengine-elevate`'s `process()` applies each operation and then appends a line. Applied in place,
`AuditLogRemove` would be followed by the line describing it — which recreates the file the operation
exists to remove — and every operation after it would do the same.

So `process()` applies `AuditLogRemove` **after every other operation in the batch**, keeping its
outcome at its own index in the response, and writes no line for it. Two passes over the batch, one
`Option<usize>`, and a comment saying why. The alternatives were both worse: refusing a request whose
ordering is wrong turns a queue that accumulated one extra row into a queue that can never be
granted again, and reordering by trusting the daemon to enqueue in the right order makes a security
property depend on a caller the helper is built not to trust.

Nothing is lost by not recording it. `PrivilegedResponse::results` carries the outcome, the daemon
logs what came back, and the report the user reads names the file and says whether it is gone.

### D6 — The helper removes itself last, and the log's directory goes with the log

Order inside the batch, which the daemon controls and the helper does not depend on:

```
hosts-apply []  ·  resolver-revoke  ·  port-access-revoke  ·  trust-ca-remove
firewall-apply []  ·  helper-remove  ·  audit-log-remove
```

`helper-remove` before `audit-log-remove` so that the last thing the audit log ever records is the
removal of the binary that writes it. `audit-log-remove` removes `elevate.log` and then its
directory — `%ProgramData%\MixEngine`, `/Library/Logs/MixEngine`, `/var/log/mixengine` — when that
directory is empty, because an empty directory in `/var/log` is still something left behind. It is
removed **only when empty**: a directory somebody else has put a file in is not ours to delete, and
`rmdir` refusing is the check rather than a walk.

`helper-remove` removes the file and, on Linux only, the `mixengine` directory that holds it
(`/usr/local/libexec/mixengine`) — again only when empty. macOS's
`/Library/PrivilegedHelperTools` and Windows' `%ProgramFiles%` are shared with the rest of the
system and are never touched; on Windows the per-product directory `%ProgramFiles%\MixEngine` is
ours and follows the file, under D8's constraint.

### D7 — Unprivileged first, privileged once, home last

The order the act runs in, and each step's reason:

1. **Stop every service**, dependents first — the walk `daemon.shutdown` already does. A database
   whose data directory is about to be deleted must be stopped politely first, and a front end
   holding port 80 must let go before the capability that let it is revoked.
2. **The user's own things**: `path.uninstall`'s mechanism, `autostart.disable`'s,
   `Certificates::remove_from_browsers`. None needs a token, all three are complete actions on their
   own, and doing them first means a declined prompt still leaves the browsers, the `PATH` and the
   login entry clean.
3. **Enqueue the privileged batch** in D6's order and, when `grant` is set, flush it — one prompt for
   everything, through `Elevation::grant_within` inside this job. `grant: false` is the two-call path
   T64 exists for: a person reads the batch before allowing it.
4. **Re-take the inventory** and set each disposition from it (D3).
5. **The home**, unless `keep_home` — D9.

### D8 — Windows cannot delete the image it is running from, and the report says so

Measured constraint, not a preference: on Windows a file with a mapped image section cannot be
unlinked, and `mixengine-elevate.exe` is running when `HelperRemove` is applied. Renaming it is
allowed; deleting it is not.

So on Windows `HelperRemove` calls `MoveFileExW(path, NULL, MOVEFILE_DELAY_UNTIL_REBOOT)` for the
file and then for `%ProgramFiles%\MixEngine`, in that order — the operating system's own removal
queue, applied in the order it was written, which is why the directory can follow the file. The
outcome is `OpOutcome::Applied` with a detail naming the restart, and the daemon turns it into
`Disposition::OnRestart` rather than `Removed`, because the file is still there and this report is a
measurement (D3).

The two Unixes unlink at once and answer `Removed`. This is the one place where "nothing is left
behind" is not true at the instant the command returns on all three systems, and it is stated in
`updates.md`, in the report a person reads, and in what the smoke test asserts — the Windows leg
asserts the file is scheduled (its name in `PendingFileRenameOperations`), not that it is gone.

Rejected: the NTFS self-delete trick — renaming the primary data stream and setting the delete
disposition. It works, and it is the technique malware uses to remove its own dropper; putting it
inside the one binary in this product that runs as root, whose stated constraint is being auditable
in a sitting, buys one file's worth of tidiness for a paragraph no reviewer should have to accept.

### D9 — The daemon removes the home as it goes, and the client is what measures it

`MIXENGINE_HOME` holds the database this daemon has open, the log it is writing, and the socket the
answer travels over. It cannot be removed from inside the call that answers.

So: the job removes everything in the home it is not holding, records the rest as `OnExit`, and
**arms** the removal on the `Api`. The RPC handler, after `jobs.begin` has returned the summary,
spawns a task that awaits that job and *then* asks whether anything was armed; only if something was
does it take a `Going` and drop it, cancelling the shutdown token. **The guard is taken after the
wait and not before**, which is the whole of why this is a spawned task rather than a guard held
across it: a grant nobody answers keeps the job open for as long as the dialog is on the screen, and
a `Going` held across that would end the daemon the moment the wait gave up. A declined grant arms
nothing, so the daemon stays up and the person can try again.

`serve` reads the armed paths after the accept loop drains; `main` performs the removal after
`Store::close` has checkpointed the write-ahead log and the home lock has been dropped, which is the
only point at which every handle this process holds inside the home is closed.

**And `mix` measures it.** Once the daemon is gone, the client re-reads the paths the report named
and prints what is left — which is what makes `mix uninstall`'s exit code mean *nothing is left
behind* rather than *the daemon said so*. That is not business logic in a client: the daemon decided
what the paths are and the client is reading them back, the same relationship `mix doctor` has with
the checks it prints.

Relocated directories are included, because `[paths]` in `config.toml` can move `runtimes/`,
`packages/`, `data/` and `logs/` to another disk — `Paths::directories()` already answers where they
really are, which is the reason overview.md promises *"the uninstaller reads their real location out
of the same file rather than assuming"*. Each one that does not lie inside the root is a
`RelocatedDirectory` row of its own rather than a second path hidden inside the `Home` row, because
the client reads these back one by one and a row is what it reads back.

### D10 — `--keep-home` exists, and the default is still complete

One flag, one disposition. Removing a home destroys the local databases in `data/` — MySQL and
Postgres instances a person may have spent a week filling — and a product whose only exit
destroys them is one people do not try in the first place. `--keep-home` undoes everything this
machine has outside `MIXENGINE_HOME` and leaves the directory; the daemon does not stop, because
there is still a home for it to serve.

The default is the complete removal the task asks for. `--dry-run` is the safety net in front of it,
and `mix uninstall` with no `--yes` runs the plan and shows it before asking.

### D11 — `mix uninstall`, not `mix daemon uninstall`

The command is about the installation and not about the daemon, and it is what a person will type
without reading anything. It sits beside `mix doctor` at the top level for that reason —
`autostart.*` and `path.*` set the precedent that a namespace exists when there are several verbs in
it, and there is exactly one verb here.

Flags: `--dry-run` (calls the plan and stops), `--keep-home`, `--yes` (skip the question),
`--no-wait` (print the job and return), `--json`.

### D12 — What the smoke test proves, and why a CI runner is the clean VM

A fresh GitHub runner has never had MixEngine on it — no hosts block, no resolver rule, no trust
store entry, no helper, no audit log. That is the clean VM the task names, and CI's `system` job
already runs elevated on all three systems for exactly this class of question.

The suite is `crates/mixengine-cli/tests/uninstall.rs`, `#[ignore]`d, and its shape is:

1. Start a daemon on a scratch home, ask for the things that need permission, grant them.
2. **Record what the machine now holds, by reading it directly** — `/etc/hosts`, `/etc/resolver`,
   `/etc/pf.conf`, the registry, `certutil`/`security`/the anchors directory, `helper_path()`,
   the audit log, the autostart entry, the shell profiles. If a system produced none of a given
   thing, that row is skipped with a reason and the test says so; it does not assert on a mechanism
   the runner does not have.
3. `mix uninstall --yes`.
4. Assert every recorded thing is gone, and that the home is gone — with the same direct reads, never
   through `mix doctor` or a MixEngine reader, because a reader that has stopped looking in the right
   place would report a clean machine either way.

A CI step per system prints the same evidence afterwards, on the shape the existing *"the trust store
this job left behind"* steps have and for their reason: what an elevated job left on a machine is
worth printing whether or not the assertions about it held.

## Data flow

```
mix uninstall
  └─ daemon.uninstall_plan { keep_home }        read-only, no prompt
       └─ uninstall::inventory::take            hosts · resolver · port access · firewall
                                                trust · browsers · helper · audit log
                                                autostart · PATH · home
  ── show the plan, ask ──
  └─ daemon.uninstall { keep_home, grant }      → JobSummary
       └─ job
            1. services stopped, dependents first
            2. PATH off · autostart off · browsers cleaned
            3. enqueue: hosts[] · resolver-revoke · port-access-revoke · trust-ca-remove
                        firewall[] · helper-remove · audit-log-remove
               grant → one prompt → mixengine-elevate
                   process(): every op, then audit-log-remove, unrecorded
            4. inventory taken again → each Disposition measured
            5. home: what can go, goes; the rest is armed and reported OnExit
       └─ Going held until the job is durable → token cancelled
  ── daemon drains, closes the store, drops the lock, removes the armed paths, exits ──
  └─ mix re-reads the named paths and prints what is left; exit 1 if anything is
```

## Testing

**Unit, unignored, on every leg:**

- `privileged.rs`: both new operations round-trip, are refused with an unknown field, share the
  dedupe keys D4 names, and require elevation.
- `mixengine-elevate`: both operations are refused under an ordinary token before they touch
  anything; `process()` applies `audit-log-remove` last and writes no line for it, with the outcome
  still at its own index; `audit::remove` refuses a directory that is not administrative, and leaves
  a directory that is not empty.
- `uninstall_api.rs`: the wire shapes, and `left_behind()` counting only `Failed`.
- `inventory`: against `mixengine-platform`'s mock `Host` — a machine holding nothing answers
  `Absent` on every row, and one holding each thing answers `Planned` with a location.

**Integration, unignored (`tests/uninstall.rs`):**

- `--dry-run` on a real home lists all eleven rows, changes nothing, and exits 0 — asserted by the
  home still being there and the queue still being empty afterwards.
- `--dry-run --json` parses and carries every `ResidueId`.
- Without `--yes` and with no answer, nothing happens and the exit code says so.
- `--keep-home` leaves the home and the daemon running, and says `Kept` for that row.

**Integration, `#[ignore]`d, CI `system` job on all three systems:** D12's round trip.

**Not tested here:** that a `.deb`'s or a `.pkg`'s own removal path works. Those are the packaging
formats' business and T85's; what this suite proves is that MixEngine takes itself off a machine.

## Risks, and where each is answered

- **The grant is declined halfway.** Every privileged row reads back as `Failed` with the store's own
  reason, the unprivileged work has already been done, and the home is not removed — `keep_home` is
  forced on when the grant did not finish, because a home removed while the machine still routes
  `.test` to a daemon that no longer exists is the worst of the states available. D3, D7.
- **The daemon dies between the grant and the home removal.** The next daemon on that home starts
  normally; the queue is empty, the machine is unwired, and `mix uninstall` run again is idempotent
  — every operation on the list is whole-state or a removal.
- **Windows leaves the helper until a restart.** D8, and it is asserted rather than hoped.
- **`remove_dir_all` fails on a file another program holds open** — a shell in another window
  holding a shim on Windows is the ordinary case. Reported as `Failed` with the path, by the client's
  own read-back, and `path.uninstall` already has the `stale` field for the same situation.
- **Two homes on one machine.** Uninstalling one takes off the hosts block, the resolver wiring and
  the trust store the other was also using. That is the honest consequence of machine-wide state that
  is not keyed by home, it is what `mix doctor --repair` on the surviving home puts back, and it is
  named in the plan the person reads. Making it correct would mean reference-counting machine state
  across homes, which is a feature and not a footnote.

## What this leaves

- **T88** (`mix self-update`) is unaffected: nothing here touches the feed, and the helper's own
  update path is T88a's.
- **T56**'s bindings gain one module. `Disposition` is internally tagged, so it describes cleanly.
- `mix doctor` gains nothing and loses nothing. Whether it should *report* a helper or an audit log
  left by a home that no longer exists is a question for whoever finds one; it is not this task's,
  because the home that would report it is the one being removed.
