# T88a — The helper's own update path (design)

Roadmap task **T88a**, phase 9: *"`mixengine-elevate` update path: excluded from auto-update, own
elevation prompt, minisign verified **inside** the elevated context, daemon↔elevate protocol
negotiation."*

Two of those four clauses are already true and one of them is true only on paper.
[T85](2026-09-04-t85-installers-design.md) gave the helper an elevation prompt of its own —
`PrivilegedOp::HelperInstall {}`, [ADR 0015](../../../.claude/decisions/0015-the-helper-installs-itself.md)
— and [T88](2026-09-04-t88-self-update-design.md) excluded it from the automatic path by name. What
neither built is the half that decides *whether a replacement deserved that prompt at all*, and
without it the helper on a machine is the helper that machine will have for ever.

That last sentence is not rhetoric. It is measured, below.

## Goal

A MixEngine that has been upgraded can also upgrade the one file it runs as root — behind an
explicit prompt, with the replacement's signature checked by the copy already installed, against a
key compiled into that copy. And a daemon newer than the helper beside it keeps working, says so,
and says what to do about it.

## Measured, not assumed

Read out of this tree on 2026-09-05.

1. **The upgrade path silently answers "already done".**
   [`elevation::choose`](../../../crates/mixengine-core/src/elevation.rs) prefers the *installed*
   helper, so the elevated process on any machine past its first prompt **is** the installed copy.
   [`helper::install`](../../../crates/mixengine-elevate/src/helper.rs) then compares
   `current_exe()` with `helper_path()`, finds them the same file, and returns
   `OpOutcome::AlreadyDone`. `AlreadyDone` deletes the queue row
   ([`elevation::settle`](../../../crates/mixengine-core/src/elevation.rs)), so the next daemon start
   asks again, is answered the same way, and nothing ever changes.
2. **Nothing puts a newer helper where the daemon could see one.**
   [`updates::apply::swap`](../../../crates/mixengine-core/src/updates/apply.rs) skips
   [`KEPT`](../../../crates/mixengine-core/src/updates/apply.rs) — so after `mix self-update`, the
   copy of `mixengine-elevate` *beside `mixengined`* is still the old one, and
   `Elevation::require_helper`'s byte comparison finds two identical old files and asks for nothing.
3. **`require_helper` compares bytes, not versions.** A `cargo build` in a development tree that
   changes one byte of the helper puts a row on `mix status` whose only meaning is "you rebuilt".
4. **The protocol is a point, not a window.**
   [`request::read`](../../../crates/mixengine-elevate/src/request.rs) refuses any request whose
   `version` is not exactly the helper's own — exit 65, no response file at all — and
   [`elevation::read_report`](../../../crates/mixengine-core/src/elevation.rs) refuses any response
   whose version is not exactly the daemon's. So *"an old elevate keeps serving the operations it
   knows"* ([`.claude/features/updates.md`](../../../.claude/features/updates.md)) is false at the
   envelope, before the per-operation tolerance `ops::decode` was written for is ever reached.
5. **The two facts that would drive a negotiation are reported and read by nobody.**
   `PrivilegedResponse::elevate_version` and `PrivilegedResponse::supported_ops` are filled in by
   [`main.rs`](../../../crates/mixengine-elevate/src/main.rs) and reach exactly one `tracing::info!`
   field in the daemon.
6. **`minisign-verify` 0.2.5 has no dependencies at all** — `Cargo.lock` carries no `dependencies`
   block for it — and it exposes `Signature::trusted_comment()`, which the global signature covers.
   That matters because [T49a](2026-08-24-t49a-system-trust-store-design.md) refused `sha2` in this
   binary over eight crates and hand-wrote a DER reader instead; the currency this crate's
   dependency list is counted in is crates, and this costs one.
7. **The signing key is only ever on the `release` job.**
   [`ci.yml`](../../../.github/workflows/ci.yml) hands `UPDATE_SECRET_KEY` to one step, in one job,
   after all five `build` legs have uploaded. No build leg can sign anything, so nothing signed can
   be inside an artifact a build leg produced.

Reading 1 is the one that makes this task urgent rather than tidy. **T88a ships in v0.1.0**
(milestone M9). A 0.1.0 that goes out without a way to replace the helper is a 0.1.0 whose helper no
later release can ever fix — and it is the only file this product runs as root.

## Scope

**In.** A second privileged operation, `HelperReplace {}`, verified inside the elevated context
against a key compiled into the running helper. The trusted comment that carries what the signature
says about the candidate. A protocol *window* on both sides, and the unelevated handshake that
discovers where the installed helper sits inside it. `elevation.upgrade` / `mix elevation upgrade`,
which fetches the candidate, proves it runs on this machine, and puts the replacement in the queue.
One new release asset per build leg and one new optional array in `latest.json`. The documents that
assert the state this changes.

**Out.** Replacing the helper without a prompt, in any form. Changing `swap`'s `KEPT` rule.
Changing which file `elevation::helper` chooses ([the T40b design's D9](2026-08-23-t40b-elevation-queue-design.md):
there is no override, and there will not be). An offline helper upgrade — see *What this leaves*.
Signing anything with the operating system's machinery, which is
[ADR 0017](../../../.claude/decisions/0017-smart-app-control-is-an-unsupported-configuration.md)'s
closed question.

## The shape

```
mix elevation upgrade
  │
  ├─ daemon: read the verified feed  ──────────────► latest.json.helpers[os, arch]
  ├─ daemon: download the helper and its .minisig ─► <home>/run/helper/
  ├─ daemon: verify (pre-check, so no prompt is spent on a bad download)
  ├─ daemon: run the candidate unelevated, one `probe` ─► it starts here, and says what it is
  └─ daemon: enqueue HelperReplace {}                    ─► "run `mix elevation grant`"

mix elevation grant
  │
  └─ OS prompt ─► the INSTALLED helper, elevated
                   │
                   ├─ read <home>/run/helper/mixengine-elevate  (bytes, once)
                   ├─ verify those bytes against PUBLIC_KEY compiled into THIS binary
                   ├─ read the signed trusted comment: version, os, arch
                   ├─ refuse older, refuse another machine's
                   └─ rename self → .old, write those same bytes, chown root, chmod +x
```

The interesting line is the one that says *those same bytes*. See D5.

## Decisions

### D1 — `HelperReplace {}` carries no fields, and the candidate lives at a fixed path under the request's home

[ADR 0015](../../../.claude/decisions/0015-the-helper-installs-itself.md) refuses
`HelperInstall { source: PathBuf }` in one line: *"it is `Exec { cmd }` with two more steps, and the
closed-enum rule in the security model exists to refuse that shape."* That reasoning is untouched
here, and the new operation obeys it: it carries nothing.

Where the candidate is, is composed by the elevated process from two things it already has — the
`home` the request names, which [`request::read`](../../../crates/mixengine-elevate/src/request.rs)
has already established belongs to the caller and contains the request file, and a constant. Both
sides compose it through one function so the two cannot drift:

```rust
// mixengine-proto, privileged.rs
pub fn helper_candidate(home: &Path) -> PathBuf;            // <home>/run/helper/mixengine-elevate[.exe]
pub fn helper_candidate_signature(home: &Path) -> PathBuf;  // …the same, plus `.minisig`
```

`run/` and not `cache/`: `Paths::new` builds `run` with `under("run", None)` — it is the one working
directory `[paths]` cannot move — and the elevated process composes from a compiled-in relative path
that a config file must not be able to redirect. It is also where `run/elevate/` already is, which
is the right neighbourhood: a staged candidate is exactly as durable as a pending elevation row.

**What a compromised daemon gains from this existing.** It can put any bytes at that path. What it
cannot do is make them verify. So the primitive is not *copy this file as root*; it is *install a
`mixengine-elevate` that MixEngine signed*, bounded further by D4. That is a different thing from
`Exec { cmd }` in the way that matters, and it is the difference this whole task consists of.

**Rejected: teaching `HelperInstall {}` to prefer a candidate when one is present.** One operation
whose meaning depends on a file the untrusted caller controls is an operation whose `describe()`
cannot tell the truth — and the screen that sentence appears on is the one whose entire job is to
say what is about to happen before somebody clicks Allow. Worse, it would turn a *first* install
into a downgrade: plant an old signed helper, wait for first-run setup, and the machine's permanent
helper is the one with the hole in it. Two operations, two sentences.

### D2 — Only the installed copy may replace itself

`helper::replace` refuses unless `current_exe()` and `helper_path()` canonicalise to the same file.

The whole value of this task is that the *trusted* copy — root-owned, in a directory an ordinary
account cannot write — is the one deciding whether a candidate deserves to be installed. A helper
running out of the user's own directory checking a signature proves nothing: whoever could replace
the helper could replace the check.

On a machine with nothing installed, `elevation::choose` runs the copy beside the daemon and the
right operation is `HelperInstall {}`, which copies its own image and checks no signature. **That is
unchanged and it is stated rather than hidden**:
[`security-model.md`](../../../.claude/architecture/security-model.md) already says malware that
replaced that copy before first run gets root once and is then installed as the permanent helper,
and that nothing but an OS signature closes it. T88a does not close it either. What T88a closes is
the *second* and every later replacement.

### D3 — `minisign-verify`, and a key compiled into the helper

`mixengine-elevate` may not depend on `mixengine-core` — `workspace_layering.rs` says so — so the
verifier and the key are the helper's own:

```toml
# crates/mixengine-elevate/Cargo.toml
# T88a. One crate, and it brings nothing with it: `minisign-verify` has no dependencies of its own.
# What it buys is the only thing standing between "the daemon said so" and "we signed it", inside
# the one process that runs as root. Ed25519 and Blake2b, both vendored in that crate.
minisign-verify.workspace = true
```

`.github/elevate-dependencies.txt` gains exactly one line, `minisign-verify`, with the paragraph
above it that the file's existing comments set the pattern for.

The key is `mixengine_elevate::PUBLIC_KEY`, pinned the same way
[`core::updates::PUBLIC_KEY`](../../../crates/mixengine-core/src/updates.rs) is, and kept honest by
the same test read at compile time:

```rust
const COMMITTED: &str = include_str!("../../../packaging/updates.pub");
```

**Two constants and one committed file, rather than one constant shared.** Moving the key into
`mixengine-proto` would give one definition, and it would also make `packaging/sign.sh` — which
`sed`s the constant out of `crates/mixengine-core/src/updates.rs` to check the Actions secret is the
pair of the committed key — read a file that is no longer the one the product pins. Each crate
pinning the same committed file, each with its own drift test, keeps every reader honest against the
same artifact and adds no cross-crate `include_str!` of somebody else's source.

**The same key as the feed's, and not a fourth one.** `core::updates`' module header already argues
this: a key of its own for the one binary that runs as root *"splits the label and not the blast
radius"* — same secret, same place, same workflow, second name.

### D4 — The trusted comment carries version, OS and architecture, and the signature covers it

`minisign` signs the trusted comment as part of the global signature, and `minisign-verify` exposes
it only after `verify` has succeeded. So it is the one place a fact about the candidate can travel
that a compromised daemon cannot write.

```
trusted comment: mixengine-elevate 0.2.0 linux x86_64
```

Parsed by `mixengine_proto::privileged::HelperStamp::parse`, in proto so that the daemon's pre-check
and the elevated check read one grammar with one set of tests. Three fields, and each one refuses
something real:

- **version** — refuse anything ordered before this build's own `CARGO_PKG_VERSION`, by
  `PackageVersion::cmp_precedence`. Without it, a compromised daemon keeps a genuinely signed helper
  from an old release and installs the hole back. `Equal` is accepted rather than refused: there is
  one published helper per version per machine, and refusing it would make a re-run of a half-done
  replacement impossible.
- **os** and **arch** — refuse another machine's. This is not hygiene: **a helper that cannot be
  loaded is a machine with no elevation at all and no way back except a reinstall**, and a correctly
  signed `aarch64` binary installed on `x86_64` is exactly that. macOS publishes one universal
  helper, so `universal` is accepted there beside the two architecture names.

**Rejected: no version at all, on the grounds that only we can sign.** "Only we can sign" bounds the
attacker to *our own past mistakes*, which is the entire content of a downgrade attack.

**Rejected: reading the candidate's version by running it.** The elevated context does not execute
anything, anywhere, and it is not going to start. The daemon does run it (D9) — unelevated, before
any prompt — but what the *root* process believes about the candidate comes only from the signature.

### D5 — Read once, verify those bytes, write those bytes

```rust
let bytes = read_at_most(&candidate, MAX_CANDIDATE)?;   // one read
verify(&bytes, &signature, PUBLIC_KEY)?;                // over the value in hand
std::fs::write(&staged, &bytes)?;                       // the same value
```

Never `verify(path)` followed by `fs::copy(path, …)`. The candidate lives in a directory the caller
owns, the caller is the party this binary is written not to trust, and a copy that re-opens the file
after the check is a check the caller can step past by swapping the file in between. This is the one
line in the module that a plausible-looking refactor would break, so it says so in a comment and a
test names it.

`MAX_CANDIDATE` is 128 MiB, refused before the read rather than after: `mixengine-elevate` is under a
megabyte on every platform this ships to, and a process running as root must not be talked into
allocating whatever a local file happens to be. Refused as `OpOutcome::Refused`, with the size in the
sentence.

Two more reads that are refusals rather than reads, on the rules `request.rs` already follows:
`symlink_metadata` — a symlink is somebody choosing which file root opens after root has decided to
trust the name — and `others_can_write`, because "the user's own home" stops meaning anything the
moment a second local account can write into it.

### D6 — The candidate is downloaded from the release, and is not taken out of the update payload

The natural-looking answer is to ship `mixengine-elevate.minisig` inside each payload archive beside
the binary. It cannot be done: reading 7 above — the signing key exists only in the `release` job,
after every build leg has finished, and an artifact cannot gain a file after it has been hashed by
`feed.sh` and signed by `sign.sh`.

Repacking the archives inside the `release` job was considered and refused: it would mean the bytes
a build leg opened and checked are not the bytes that ship, and it reaches none of the five
installers anyway.

So each build leg additionally publishes the raw helper as its own release asset, which `sign.sh`
then signs like everything else in `dist/` — no new signing machinery at all:

| Leg | Asset |
| --- | --- |
| Windows | `mixengine-elevate-<version>-windows-<arch>.exe` |
| macOS | `mixengine-elevate-<version>-macos-universal` (the `lipo`d one, the same file the `.pkg` carries) |
| Linux | `mixengine-elevate-<version>-linux-<arch>` |

and `latest.json` gains an array naming them:

```json
"helpers": [
  { "os": "linux", "arch": "x86_64", "url": "https://…/mixengine-elevate-0.2.0-linux-x86_64", "size": 812345 }
]
```

`#[serde(default)]`, so a feed written before this field still reads and
[`feed::SCHEMA`](../../../crates/mixengine-core/src/updates/feed.rs) does not move — that file
already states the rule: *"Bumped only for a change an existing client cannot read. Adding an
optional field is not one."* The signature's URL is the asset's plus `.minisig`, which is the
convention `index::Client` already appends and `sign.sh` already writes.

macOS produces two rows pointing at one URL, exactly as its payload archive does, for
[T88's D6](2026-09-04-t88-self-update-design.md) reason: a caller asks with the pair it already has
rather than learning what "universal" means.

**What this buys beyond feasibility.** The helper a machine installs is one that was *published*,
rather than whatever happens to be lying beside `mixengined`. There is no path by which a machine
talks itself into installing a helper nobody released.

### D7 — The handshake is an unelevated run of the installed helper

The daemon needs three facts about the installed helper before it can decide anything: which
protocol it speaks, which version it is, and which operations it knows. All three are already in
every `PrivilegedResponse`, and none of them needs a token — `Probe` is the operation
[the T40 design's D5](2026-08-22-t40-elevate-design.md) made applicable without one, precisely so the
answer to "is this process elevated" could ever be `false`.

So: at start, when a helper *is* installed, `mixengine-daemon` runs it as an ordinary process with a
one-operation `probe` batch in a fresh single-use directory under `run/elevate/`, marked
`PROTOCOL_MINIMUM`, with a 5-second timeout, and keeps what comes back:

```rust
struct HelperFacts {
    speaks: ProtocolVersion,   // response.version
    version: String,           // response.elevate_version
    supported_ops: Vec<String>,
}
```

Every failure is `None` and nothing fails the start, on the rule every block around `require_helper`
in `main.rs` already follows. In particular a handshake that meets `EXIT_UNAVAILABLE` because a
grant holds `elevate.lock` is `None` and not an error.

**No subprocess on a machine with no helper installed**, which is every first run and every
development tree — there the daemon has nothing to ask and `require_helper` enqueues
`HelperInstall {}` as it does today.

**Rejected: remembering the last grant's response in `settings`.** It is free, and it is stale
exactly when it matters: a `.pkg` or a `.deb` reinstall replaces the helper underneath the record,
and a keyed-by-`mtime` invalidation is a cache that answers "unknown" in the case the whole feature
is about. One process that answers truthfully beats one row that sometimes does.

**Rejected: raising a prompt to find out.** Spending an elevation prompt to discover what the
installed binary is, is the shape `supported_ops` was added to avoid — its own doc comment says so.

### D8 — A protocol window, and the daemon speaks down to it

```rust
/// What this build speaks.
pub const PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);

/// The oldest peer this build still serves.
pub const PROTOCOL_MINIMUM: ProtocolVersion = ProtocolVersion(1);
```

- **The helper** accepts `PROTOCOL_MINIMUM ..= PROTOCOL_VERSION`. Below the floor is a daemon older
  than this helper still serves; above the ceiling is a daemon that did not speak down, and is
  refused rather than guessed at — the version number exists so that a fixed old binary can refuse
  what it cannot be sure it understands, and an old binary can never be taught otherwise later.
- **The daemon** marks a request `min(PROTOCOL_VERSION, facts.speaks)`, and `PROTOCOL_VERSION` when
  it has no facts — which is the case where the file being elevated is the copy shipped beside this
  very build.
- **`read_report`** accepts any `response.version` at or above `PROTOCOL_MINIMUM` instead of exactly
  `PROTOCOL_VERSION`. `version` keeps its meaning — *what the helper speaks* — so a newer helper
  answering an older daemon stays readable, which is what the response's deliberate absence of
  `deny_unknown_fields` was always for. The reply is bound to *this* request by the nonce, which is
  already checked two lines above.

Both constants are 1 today, so **nothing about today's wire behaviour changes**. What changes is
that the rule is written down, is tested from both sides, and is the thing a protocol 2 will be able
to rely on — which is the only moment it could ever be introduced, since the helper it has to be
compatible with is the one shipping now.

**And it is not speculative machinery**, because the facts it produces have a caller today: D10.

### D9 — `mix elevation upgrade` runs the candidate before it queues anything

Between "the update was refused and nothing changed" and "MixEngine no longer starts",
[T88's smoke test](2026-09-04-t88-self-update-design.md) is the difference. The same sentence holds
one step further in: between *"the upgrade was refused"* and *"this machine can no longer elevate
anything"*, the difference is running the candidate once, here, as an ordinary process.

It is the same mechanism as D7's handshake pointed at the staged file, and it answers exactly the
things a signature cannot: a Windows Code Integrity refusal
([`.claude/features/updates.md`](../../../.claude/features/updates.md) records `os error 4551` as a
refusal rather than a warning, re-judged after every update, per file), a Linux build past this
machine's glibc floor, and a binary that will not load for a reason nobody predicted.

If it will not run here, nothing is enqueued and the reason is reported. The candidate is left on
disk for a person to look at; the next `mix elevation upgrade` removes it before staging again, on
`updates::apply::stage`'s rule about a staging directory left by an attempt that was killed.

### D10 — What the daemon does with an old helper, and what it says

`Elevation::require_helper` stops comparing bytes. Four answers:

| Installed | This daemon | What happens |
| --- | --- | --- |
| nothing | — | `HelperInstall {}` enqueued, as today |
| version ≥ this build's | — | nothing |
| version < this build's, and `supported_ops` holds `helper-replace` | | nothing enqueued; `elevation.status` carries *"the privileged helper on this machine is 0.1.0 and MixEngine is 0.2.0 — `mix elevation upgrade` fetches the newer one and asks"* |
| version < this build's, and `supported_ops` does **not** hold it | | nothing enqueued; `elevation.status` carries *"the privileged helper on this machine is 0.1.0, which cannot replace itself. It still serves everything it knows. Reinstalling MixEngine with its installer replaces it."* |

That last row is *"an old elevate keeps serving the operations it knows while the app asks the user
to upgrade it"*, made of code. It is also why D7 and D8 are not speculative: without
`supported_ops`, the daemon's only way to discover that row is to enqueue `helper-replace`, spend a
prompt, and be told `Unsupported` — which then deletes the row
([the T40b design's D5](2026-08-23-t40b-elevation-queue-design.md)) and leaves the user with a
refusal and no sentence.

**Nothing here reaches the network**, which is why the enqueue moved out of `require_helper` and into
`elevation.upgrade`: a daemon start that downloaded a binary would be a start an offline machine
pays for, which `.claude/features/updates.md` forbids in as many words.

### D11 — Windows replaces by renaming, and the `.old` is cleaned by the next elevated run

A file whose image is mapped cannot be unlinked or written on Windows, and the helper *is* the
running program when it replaces itself. It can, however, be renamed —
`updates::apply::swap` already leans on exactly this for `mix.exe` and `mixengined.exe`, and this
follows it:

1. `rename(destination, destination + ".old")`;
2. write the verified bytes to a `.new` beside the destination, `own_as_root`, `make_executable`;
3. `rename(.new, destination)`;
4. on any failure after step 1, remove what step 2 wrote and rename `.old` back.

`own_as_root` **before** the rename and not after, which is `helper::place`'s existing rule and its
reason: nothing is ever reachable at the destination that is not already root's, and on macOS
`fs::copy` carries the source's owner across.

The `.old` is removed best-effort at the start of the next elevated `helper::install` or
`helper::replace` — by then the process that renamed itself has exited and its image is unmapped —
and by `helper::remove`, so [T87](2026-09-04-t87-uninstall-design.md)'s *"nothing is left behind"*
keeps meaning what it says and the `system` job's helper-directory check stays green.

On Unix the rename is unnecessary and is done anyway: one code path, one set of tests, and the
`.old` is the only way back on the platform that has no other.

### D12 — `elevation.upgrade`, one method and one subcommand

`mix elevation …` is documented as *"one subcommand per `elevation.*` method, and nothing that is
not one"*, so this is one of each: `elevation.upgrade` and `mix elevation upgrade`.

```rust
pub struct HelperUpgrade {
    /// What is installed now, when the handshake could read it.
    pub installed: Option<String>,
    /// The helper the feed offers, when it offers one for this machine.
    pub offered: Option<String>,
    /// What happened, and why when it is not the first one.
    pub outcome: HelperUpgradeOutcome,
    /// The queue afterwards, so the client prints what will be asked for.
    pub pending: Vec<PendingOp>,
}

pub enum HelperUpgradeOutcome {
    Staged,                        // downloaded, verified, run here, and queued
    UpToDate,
    Unsupported { reason: String }, // the installed helper cannot replace itself
    Unavailable { reason: String }, // no feed, no helper for this machine, or it will not run here
}
```

The command prints what is queued and says `mix elevation grant` applies it — which is the idiom
every producer in this product already follows: creating a site enqueues a hosts change and tells
you to grant it. A command that raised the prompt itself would be a second door into one, and
`elevation.grant` is deliberately the only one.

**A copy of MixEngine a package manager installed is refused in words**, reusing
[`updates::Placement`](../../../crates/mixengine-core/src/updates/placement.rs) and its sentence: a
`.deb`, an `.rpm` or a `.pkg` put the helper at that path as root, and the same package manager
replaces it.

### D13 — The daemon verifies too, and that is not a duplicated check

The signature is checked in the daemon before anything is staged, and again inside the elevated
process. The second one is the security boundary; the first one is the user interface. Without it,
a mirror that answered with rubbish would cost an elevation prompt to discover — and the acceptance
criterion in `.claude/features/updates.md` is *"a tampered artifact fails the minisign check and is
refused, **with the reason shown**"*, which is a sentence somebody has to be able to read without
having clicked Allow first.

`core::updates::helper::verify` takes the public key as a parameter, on
[`blueprints::trust::verify`](../../../crates/mixengine-core/src/blueprints/trust.rs)'s precedent and
for its reason: a compiled-in key cannot answer *does verification refuse everything else*, because
no test can produce a signature under it. `mixengine-testkit`'s `Signer` gains a trusted comment so
both halves are exercised with a key a test owns.

## What is written where

| Crate | What |
| --- | --- |
| `mixengine-proto` | `PROTOCOL_MINIMUM`; `PrivilegedOp::HelperReplace {}` and its `ALL`/`name`/`describe`/`dedupe_key`/`requires_elevation` arms; `helper_candidate`/`helper_candidate_signature`; `HelperStamp`; `HelperUpgrade`, `HelperUpgradeOutcome`, `InstalledHelper`; `ElevationStatus::installed_helper`; `rpc::method::ELEVATION_UPGRADE` |
| `mixengine-elevate` | `minisign-verify`; `PUBLIC_KEY` and its drift test; `helper::replace`; the `.old` sweep; the protocol window in `request::read` |
| `mixengine-core` | `updates::helper` — fetch, verify, stage; the protocol window in `read_report`; `write_request` takes the version to mark |
| `mixengine-daemon` | the handshake; `require_helper`'s new table; `elevation.upgrade`; `installed_helper` on `elevation.status` |
| `mixengine-cli` | `mix elevation upgrade` and its rendering; the helper line on `mix elevation status` |
| `mixengine-testkit` | `Signer::sign_with_comment` |
| `packaging/` | the raw helper asset per leg; `sign.sh --version` and its trusted comments; `feed.sh`'s `helpers` array; `feed-check.sh` |
| `.github/` | one line in `elevate-dependencies.txt` |

## Tests

**Refusals, in `mixengine-elevate`, with a key a test owns** (`minisign` as a dev-dependency, on
`rcgen`'s precedent — it never enters the closure `lint` diffs, and `cargo tree --edges normal` does
not see it):

- a candidate whose signature does not verify;
- a candidate whose signature verifies but whose trusted comment names an older version;
- …another operating system, and another architecture;
- …a trusted comment that is not the grammar at all;
- a candidate that is a symlink, and one another account can write;
- one larger than the cap;
- `helper-replace` under an ordinary token — the gate, beside the seven already in `ops.rs`;
- `helper-replace` when the running process is not the installed copy (D2).

**The window, in `tests/protocol.rs`**, against the real binary and under whatever token the suite
has: a request marked `PROTOCOL_MINIMUM` is served; one above `PROTOCOL_VERSION` is refused with
exit 65 and no response file; one below the floor likewise. And `PROTOCOL_MINIMUM <=
PROTOCOL_VERSION`, in proto, which is the assertion that catches a floor raised past the ceiling.

**The positive path, in `crates/mixengine-elevate/tests/system.rs`**, `#[ignore]`d and run by the
`system` job's elevated leg, on the same rules the helper-install test there already follows: it
skips when the machine running it is a workstation with a helper of its own.

**The daemon's table (D10)** as unit tests over `HelperFacts` values, which is what makes the four
rows exercisable on a developer machine that has exactly one of them.

**`mix self-update` still keeps the helper.** The existing test
`the_elevated_helper_is_never_replaced_and_is_reported_as_kept` is untouched and is now the thing
that stops this task from having quietly widened the auto-update boundary.

**Packaging**, in `packaging/feed-check.sh`: the fixture distribution gains a raw helper asset, and
the check asserts `helpers[]` names it with the right pair — the same shape of check that caught
T85c's `provides` key.

## What this leaves

**An offline machine cannot upgrade its helper.** The candidate comes from the release, so a person
who installed 0.2.0 from a zip on a machine with no network keeps 0.1.0's helper — working, serving
everything it knows, and saying so. The `.deb`, the `.rpm` and the `.pkg` do not have this problem
because they place the helper as root at install time. A `--from <path>` on
`mix elevation upgrade` would close it, and is deliberately not built: it is a flag that chooses
which file is a candidate for running as root, and the signature makes it *safe* rather than
*obviously safe*, which is not the same standard. It ships the day somebody has the machine that
needs it.

**Rotating the updater key is now a heavier one-way door**, and this is the sentence
`.claude/features/updates.md` gains. Every installed helper pins exactly one key, compiled in; after
a rotation, no helper installed before it will ever accept a candidate again, and the only way to
replace one is a package manager running as root. The mitigation is the one that page already
describes and has not needed — accepting a set of keys rather than one — and the cost of not having
built it just went up by one binary.

**The first prompt on a fresh machine is still unchecked**, which is D2's second half and
[`security-model.md`](../../../.claude/architecture/security-model.md)'s stated residual. That
document's line — *"the only thing that closes it is a signature the operating system checks before
the prompt: T94's question, and T88a's check"* — needs its second half corrected rather than ticked:
T88a's check closes every replacement after the first and does not close the first.

## Documents this changes

- **A new ADR**, `0018-a-signed-candidate-is-what-lets-a-path-cross-the-boundary.md`. ADR 0015
  refused `HelperInstall { source }` and its reasoning is being *extended* rather than edited, which
  `CLAUDE.md` says is a new record's job: what makes a path acceptable here is not that the caller is
  trusted — it is not — but that the bytes it points at must carry a signature the elevated process
  checks itself, against a key it was compiled with.
- [`.claude/features/updates.md`](../../../.claude/features/updates.md) — *"What must never
  auto-update"* becomes built rather than promised; the rotation paragraph gains the sentence above;
  the acceptance criteria gain the helper's own.
- [`.claude/architecture/security-model.md`](../../../.claude/architecture/security-model.md) — both
  places that say the check is *"not built yet"*, and the residual's second half.
- [ADR 0015](../../../.claude/decisions/0015-the-helper-installs-itself.md) — its *Consequences*
  section names T88a twice as the thing still ahead; those two sentences point at the new ADR.
- [`.claude/roadmap/phase-9-ship.md`](../../../.claude/roadmap/phase-9-ship.md) — T88a ticked, with
  what the implementation changed about its own sentence.
- [`packaging/README.md`](../../../packaging/README.md) — the sixth asset per leg.
