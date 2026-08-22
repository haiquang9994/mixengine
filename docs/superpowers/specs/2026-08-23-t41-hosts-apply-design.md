# T41 — `HostsApply`: the first privileged operation with an effect

*Design, 2026-08-23. Roadmap task [T41](../../../.claude/roadmap/phase-4-sites-and-elevation.md), Phase 4.*

## What this closes

[T40](2026-08-22-t40-elevate-design.md) built the helper and its file protocol,
[T40a](2026-08-22-t40a-elevation-design.md) the capability that raises the prompt,
[T40b](2026-08-23-t40b-elevation-queue-design.md) the queue that decides when a prompt is worth
spending, and T64 the screen that comes before it. Every one of them shipped with the same hole in
the middle: **nothing in this workspace asks for anything.** `Probe` changes nothing by design, and
the one row every elevation suite runs against is written by a fixture,
`mixengine_testkit::privileged`, which has carried this task's name in its doc comment since the day
it was written.

T41 is the operation that closes it — the first member of `PrivilegedOp` with an effect, and the
first producer of the queue. When it lands, a test that creates a site and *then* finds an operation
waiting proves what the fixture cannot: that the queue is filled by the product.

It is also the first code MixEngine ships that edits a file the operating system owns, which is why
the task line names one test before it names anything else. **Unrelated lines survive.** That
regression is the one users never forgive, and it is the acceptance criterion this whole design is
arranged around.

No ADR is needed. `HostsApply` is already on the closed list in
[platform-abstraction.md](../../../.claude/architecture/platform-abstraction.md#privileged-operations);
the rule exists to stop a *new* capability being granted quietly, and this is the one the list was
written for.

## What already exists, and is reused unchanged

- The whole elevation stack: `PrivilegedRequest`/`PrivilegedResponse`, the per-index `OpOutcome`, the
  helper's own request validation, the audit log, `Elevation::run`, the queue, `elevation.grant`,
  and `mix elevation status|grant|drop`. This task adds one enum variant and one producer; it adds
  no mechanism.
- `mixengine_platform::lock::Lock`, which takes an arbitrary path and whose handle *is* the lock.
- `mixengine_platform::elevated::audit_directory`, the one directory MixEngine already keeps outside
  `MIXENGINE_HOME`, root-owned and machine-wide.
- `mixengine_core::domains`, which decides what a domain may be. Its table moves (D4); its policy
  does not.
- `Elevation::enqueue` in the daemon, written by T40b with `expect(dead_code)` and a comment naming
  this task. The attribute comes off.

## Decisions

### D1 — The operation carries the whole managed block, not a delta

`HostsApply { entries: Vec<HostEntry> }` is the state the block should be in when the helper is
finished, not a list of changes to make to it.

The delta shape composes better and heals nothing. A block that has drifted — a user edited it, a
crash left it half written, another home wrote its own — cannot be pulled back by "add this line",
and `mix doctor` (T47) would have to derive the subtraction itself from two readings it does not
have. A whole-state operation is idempotent, is its own repair, and makes the `AlreadyDone` outcome
a byte comparison rather than a judgement.

It also matches the standing rule that generated config is disposable: the block is rendered from
the database every time and never parsed back into state.

### D2 — Its dedupe key is its *kind*, and a newer state supersedes an older one

T40b made `dedupe_key` the operation's serialisation, so two identical requests are one row. That is
right for `Probe` and wrong for a whole-state operation: create two sites before anybody clicks
Allow and the queue holds two `hosts-apply` rows, both valid, disagreeing, and both rendered on the
screen whose entire job is to say what is about to happen.

So `dedupe_key` becomes the operation's **identity**, which for `Probe` is still its serialisation
and for `HostsApply` is the bare kind, `hosts-apply`. The insert becomes an upsert with a guard:

```sql
INSERT INTO pending_privileged_ops (op, dedupe_key, requested_at)
VALUES (?, ?, ?)
ON CONFLICT (dedupe_key) DO UPDATE SET op = excluded.op WHERE op <> excluded.op
```

`requested_at` is deliberately not refreshed: the need started when it started, and a queue that
reset its own clock on every site creation would report a wait that never got older.

`rows_affected` keeps meaning exactly what T40b's caller already reads it as — nothing changed, so
nothing is announced. The `WHERE` clause is what preserves that: re-enqueueing the same desired
state touches no row and publishes no `ElevationRequired`.

No migration. The column and its unique index already exist; only the value computed for it changes,
and `Probe`'s value does not change at all.

### D3 — Mechanism in `mixengine-platform`, policy in `mixengine-elevate`

The marker-block engine — find the block, splice it, render it, replace the file atomically — is
`mixengine-platform`'s, because it ends in `ReplaceFileW` on one system and `fchown` on the others
and the standing rule is that no OS call lives outside that crate. It is compiled under both `host`
and `elevated`, so the helper reaches it without pulling in tokio or a keyring.

The decision about *what may be written* is `mixengine-elevate`'s, because that is the binary that
must not trust its caller. `src/hosts.rs` there is the whole of the policy and can be read in one
sitting, which is the property the security model asks of this binary.

The split is not cosmetic: it is what lets the dangerous half be tested exhaustively against files a
test owns, while the half that could turn `evil.com` into loopback sits in forty lines with nothing
else in them.

### D4 — The managed TLD table moves to `mixengine-proto`

The helper has to refuse a domain outside a managed TLD — security model, request validation, rule 2
— and `core::domains` says, in its own doc comment, that a second copy of "is this a domain?" is a
second answer to a question with exactly one. Both are right, and the way to keep both is to move
the *table* rather than duplicate the *check*.

`MANAGED_TLDS`, `DEFAULT_TLD` and a pure syntax predicate go to `mixengine-proto`. `core::domains`
keeps every bit of policy that is not the table — `.local` needing `accept_risky_tld`, the wording of
each refusal, which error each becomes — and reads the table from proto. `mixengine-elevate` calls
the predicate **itself**, on the data in the request.

Sharing a compile-time constant is not trusting the daemon; being handed a list of permitted TLDs in
the request would be, and that is the shape this rules out. A client gets the table too, which is
what `.local`'s warning needs anyway.

The helper being excluded from auto-update means its table can be older than the daemon's. That is
the correct failure: a TLD a future build manages is refused by the installed helper, loudly, at its
own index — never applied because the caller said it was fine.

### D5 — Loopback only, and one line per domain

Every entry's address must be `127.0.0.1` or `::1`. Nothing MixEngine does needs a hosts entry
pointing anywhere else — LAN sharing rebinds a listener and adds certificate SANs, it does not
publish names — and an unconstrained address is precisely the hosts-file hijack that Defender has a
heuristic for.

The producer emits `127.0.0.1` alone, one line per domain.
[domains-and-dns.md](../../../.claude/features/domains-and-dns.md) draws its example block with a
matching `::1` line, and that example is wrong for today's build: nothing decides that the web server
binds `::1` until T43, and a name that resolves to an address nothing is listening on is a browser
timing out before it retries. The feature document is corrected as part of this task rather than
left to contradict the code. `::1` stays permitted by the helper so T43 can start emitting it without
touching the audited binary.

### D6 — A malformed block is refused, never repaired by guessing

The engine matches `# BEGIN MixEngine` and `# END MixEngine` against a trimmed line, exactly. Then:

| What is found | What happens |
| --- | --- |
| Neither marker | The block is appended, after a newline if the file did not end with one |
| One `BEGIN`, one `END` after it | The lines between them are replaced |
| `BEGIN` with no `END` | Refused |
| `END` with no `BEGIN` | Refused |
| A second `BEGIN` anywhere | Refused |
| An empty entry list | The marker lines and everything between them are removed |

A refusal is `OpOutcome::Refused`, which says the same request will be refused again — correct here,
because what is wrong is on the machine and a person has to look at it. The alternative, picking one
of the two `BEGIN`s and writing between them, is a program editing a system file according to a guess
about what somebody else meant.

Everything outside the block is copied byte for byte. The block itself is rendered with the file's
own line ending — CRLF if the rest of the file uses it anywhere — because rewriting a Windows hosts
file with Unix endings is a diff on every line, in a file people read with Notepad.

### D7 — The write is a replace, and it carries the old file's permissions

Temp file in the *same directory*, then swap. Same directory, because a rename across filesystems is
a copy and is not atomic.

On Unix the mode, uid and gid are read from the file being replaced and applied to the temp file
before the rename; the directory is fsynced after it. Skipping that is how a `0644 root:root`
`/etc/hosts` quietly becomes something wider, and nothing would report it. On Windows the swap is
`ReplaceFileW`, which preserves the ACL, the attributes and the creation time that a plain rename
discards — the architecture document already names it for this reason.

**No backup file.** The rename is atomic, so there is no torn state to recover from, and a
`hosts.mixengine.bak` left in `/etc` is litter that outlives the reason for it. The reverse operation
already exists and is `HostsApply { entries: [] }`.

If the file is absent entirely — not a state any of the three systems ships in, but reachable — it is
created with the mode `/etc/hosts` normally carries.

### D8 — A second lock, machine-wide, and the limitation it does not fix

The helper already holds `<home>/run/elevate.lock` for its whole run. That serialises two helpers
*for one home* and says nothing about two homes: two accounts on one machine, each with its own
`MIXENGINE_HOME`, both editing `/etc/hosts`.

So the write takes a second lock, `hosts.lock`, in the audit directory — already root-owned, already
machine-wide, already created by this binary on first run. Taken for the read-modify-write and
dropped after the rename. A lock somebody else holds is `OpOutcome::Failed`: nothing about the
request is wrong and trying again will work.

**What the lock does not fix, and this design does not fix:** the two homes share one
`# BEGIN MixEngine` block, so the second one's desired state replaces the first one's rather than
merging with it. Per-account markers would fix it and would make the block unreadable and
`mix doctor` a great deal harder. It is recorded in the phase file as a known limitation of a
multi-account machine, which is not the machine this product is for.

### D9 — `HostsFile` on `Host` is read-only

The architecture table describes `HostsFile` as "add/remove/list managed entries". The add and the
remove cannot live there: they need an administrative token, and a capability the daemon can call is
by definition one it holds no token for. So the trait is `path()` and `managed()` — reading
`/etc/hosts` needs no privilege on any of the three systems — and the write is the privileged
operation. The table is corrected.

This is not a trait invented for symmetry. D11 needs it, `domain.dns_status` (T46) needs it to
answer "hosts entry present?", and `mix doctor` (T47) needs it to reconcile. `mock::HostsFile`
answers from memory, which is what makes D11 testable without a machine.

### D10 — Every site's domains, disabled or not

`core::hosts::desired` renders one entry per domain of every site in the home, whatever `sites.state`
says.

A disabled site is one that is not *served*; the hosts block is about *name resolution*. Excluding
disabled sites would mean `site.disable` and `site.enable` each cost an elevation prompt, which is a
password dialog for a state change that touches nothing on disk — the elevation budget in the
security model is explicit that a prompt is a cost. A disabled name resolving to loopback and being
refused by the web server is a better failure than a name that does not resolve, because the first
one is diagnosable.

### D11 — The producer reads the disk before it spends a prompt

`Elevation::require_hosts(desired)` in the daemon: read the current block through `HostsFile`, and
enqueue only if it differs from what the database says it should be. Identical means the machine
already needs nothing, and enqueueing anyway would put a row on `mix status` whose only possible
outcome is a prompt answered `AlreadyDone`.

It lives on `Elevation` rather than on `Sites` because `Elevation` is already constructed with
`Arc<dyn Host>` and already owns the "is this worth a prompt" question. `Sites` gains one dependency
and three call sites — after a successful `create`, `update` and `delete`, and never before, so a
failed create asks for nothing.

A read that fails is not a reason to refuse the site. It is logged and the operation is enqueued: the
helper is the authority on what is in that file, and it will say `Refused` with the reason on the
screen T64 built, which is a better place for "your hosts file has two BEGIN markers" than a site
creation's error.

## The interface

**`mixengine-proto`**, in `privileged`:

```rust
pub struct HostEntry {
    pub address: IpAddr,
    pub domain: String,
}

pub enum PrivilegedOp {
    Probe {},
    HostsApply { entries: Vec<HostEntry> },
}

impl PrivilegedOp {
    /// Sorted and deduplicated, so two orderings of one change are one operation.
    pub fn hosts_apply(entries: impl IntoIterator<Item = HostEntry>) -> Self;

    /// The identity a queue deduplicates on — D2. `Probe`'s is its serialisation; a whole-state
    /// operation's is its kind.
    pub fn dedupe_key(&self) -> String;
}
```

`describe()` for `HostsApply` lists what will be written, in full and in order, because the screen it
renders on exists to be read before somebody clicks Allow: *"point 2 names at 127.0.0.1 in the hosts
file: blog.test, api.blog.test"*, and for the empty list, *"remove MixEngine's block from the hosts
file"*.

and, moved from `mixengine-core::domains`:

```rust
pub const DEFAULT_TLD: &str = "test";
pub const MANAGED_TLDS: [&str; 3] = ["test", "localhost", "local"];

/// Syntax alone — RFC 1035 labels, length limits, no wildcard. Says nothing about policy.
pub fn is_domain_syntax(name: &str) -> bool;
```

**`mixengine-platform`**, new module `hosts`, under `host` and `elevated`:

```rust
pub const BEGIN_MARKER: &str = "# BEGIN MixEngine";
pub const END_MARKER: &str = "# END MixEngine";

/// Where this OS keeps the file.
pub fn path() -> PathBuf;

/// The entries in the managed block of `text`, or why the block cannot be read.
pub fn parse(text: &str) -> Result<Vec<HostEntry>>;

/// `text` with the managed block set to `entries`, or why it cannot be.
pub fn splice(text: &str, entries: &[HostEntry]) -> Result<String>;

/// Read, splice, replace — atomically, under the machine-wide lock. `elevated` only.
pub fn apply(path: &Path, entries: &[HostEntry]) -> Result<Change>;

pub enum Change { Written { entries: usize }, Unchanged }
```

`apply` takes its path rather than calling `path()`, which is what lets every test above the unit
level drive the real engine against a file it owns.

One new variant on `platform::Error`, so the helper can tell a refusal from a failure:

```rust
MalformedBlock { path: PathBuf, reason: String },
```

**`traits/hosts.rs`**:

```rust
pub trait HostsFile: Send + Sync {
    fn path(&self) -> PathBuf;
    fn managed(&self) -> Result<Vec<HostEntry>>;
}
```

on `Host`, with a `mock::HostsFile` seeded by `mock::Host::with_hosts`.

**`mixengine-core`**, new module `hosts`:

```rust
/// One `127.0.0.1` entry per domain of every site, sorted — D1, D5, D10.
pub async fn desired(store: &Store) -> Result<Vec<HostEntry>>;
```

and `elevation::canonical` splits into the encoded operation and `PrivilegedOp::dedupe_key`, with the
guarded upsert of D2.

**`mixengine-daemon`**: `Elevation::require_hosts`, the `expect(dead_code)` removed from
`Elevation::enqueue`, and `Sites` constructed with `Arc<Elevation>`.

**`mixengine-elevate`**, new module `hosts`: validate, then call `platform::hosts::apply`, then map
its answer onto an `OpOutcome`.

| What the helper finds | Outcome |
| --- | --- |
| An address that is not loopback, a domain that fails syntax or is outside the managed TLDs, more than 512 entries | `Refused` |
| A block with two `BEGIN`s, or a `BEGIN` with no `END` | `Refused` |
| The file already says exactly this | `AlreadyDone` |
| Written | `Applied` |
| The lock is held, or the OS refused the write | `Failed` |

The cap is not a formality: an unbounded list from a compromised daemon is a denial of service
against every name lookup the machine makes, and it costs one comparison to make unreachable.

## Crate changes

**`mixengine-proto`** — `HostEntry`, one enum variant, `hosts_apply`, `dedupe_key`, and the domain
table arriving from core. No new dependency; `IpAddr` is `std`.

**`mixengine-platform`** — `src/hosts.rs`, `src/traits/hosts.rs`, `src/mock/hosts.rs`, and the path
itself in `windows/hosts.rs` and `unix/hosts.rs` — the second named by `macos/mod.rs` and
`linux/mod.rs`, which is what `unix/` is for. One `Error` variant, one accessor on `Host`. The `elevated`
feature gains `dep:mixengine-proto`, which adds **no crate** to the helper's closure: it is already a
direct dependency of `mixengine-elevate`. `ReplaceFileW` is in `Win32_Storage_FileSystem`, already
enabled for the lock's share mode.

**`mixengine-core`** — `src/hosts.rs`, the table leaving `domains`, the upsert in `elevation`. A
`sqlx::query!` changes, so `cargo sqlx prepare` runs.

**`mixengine-daemon`** — `require_hosts`, three call sites in `sites.rs`, one field.

**`mixengine-testkit`** — `src/privileged.rs` is **deleted**, with the module line and the note in
`todo.md` that promised it.

**No change** to `mixengine-cli`, `mixengine-supervisor` or `mixengine-shim`. T64 already renders
whatever `describe()` returns, which is the whole of the client half.

## Testing

**The regression the task line names, in `mixengine-platform`.** A hosts file with everything a real
one has — comments, `127.0.0.1 localhost`, `::1 ip6-localhost`, tabs, blank lines, another product's
marked block, no trailing newline — is spliced, then spliced again with a different set, then spliced
empty. After the last one the file is **byte-identical** to the original. Every unrelated line
survives every step, which is asserted by comparing the file minus our block against the original
rather than by looking for the lines we expected to keep.

Around it: CRLF preserved, a file with no markers appended to, each of D6's three refusals, an entry
set that differs only in order producing an identical file, and `Unchanged` when it already matches.

**The atomic replace, against a file the test owns.** Mode and owner survive on Unix; the ACL survives
on Windows, asserted the way `access.rs` already asserts one. A temp file is not left behind on the
refusal paths.

**Validation, in `mixengine-elevate`.** `evil.com`, `8.8.8.8`, `BLOG.TEST`, `*.blog.test`, the empty
domain, 513 entries — each `Refused`, each naming what it broke. And `blog.test`, `x.localhost`,
`printer.local` accepted, because a refusal test that never says yes proves only that the code
refuses.

**Dispatch, in the daemon, against `mock::Host`.** `with_hosts` seeded to what the database says →
`require_hosts` enqueues nothing. Seeded to something else → one row, one `ElevationRequired`. Create
two sites → still one row, holding the *second* state, and only one event, which is D2 asserted
rather than described.

**End to end, over a real socket.** `crates/mixengine-daemon/tests/elevation.rs` creates a site and
then finds a `hosts-apply` waiting whose description names the domain — the test T64's notes said
would replace the fixture — and `crates/mixengine-cli/tests/elevation.rs` builds its queue the same
way instead of writing a row. Both use a domain unique to the run, so the comparison against the
machine's real hosts file cannot accidentally match.

**Nothing in any suite grants**, on T40a's and T64's precedent: a successful grant is a real dialog
on the machine running `cargo test`. What is new here is that a grant would also *edit that machine's
hosts file*, which makes the rule stricter rather than merely inherited.

**One `#[ignore]`d system test**, for the elevated per-OS job that T40 created: the real path, the
real file, applied and then removed, with the file compared against a copy taken before the run.
`mixengine-platform`'s own `tests/` is where it goes, beside `elevated.rs`.

## Out of scope, and where each goes

| Not here | Where |
| --- | --- |
| Whether an unsigned binary is allowed to make this write at all, under Smart App Control and Defender's `HostsFileHijack` heuristic | T41a |
| Rendering a site's config and serving it, which is what makes the name useful | T43 |
| The DNS server that makes hosts entries the fallback rather than the mechanism | T44, T45 |
| `domain.*` and `domain.dns_status`, the second reader of `HostsFile` | T46 |
| Hosts-only mode reported as a distinct mode on the API | T46a |
| Reconciling the block against reality, and flushing what a decline left behind | T47 |
| Removing the block at uninstall | T92 |
| Per-account markers for a multi-account machine | Nowhere yet — D8 records it as a limitation |
