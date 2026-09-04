# T88 — `mix self-update`, and the one command that outlives the daemon (design)

Roadmap task **T88**, phase 9: *"Auto-update, MixEngine's own: `mix self-update` against
`latest.json` on GitHub Releases via the stable asset URL (not the API), signature verified before
the JSON is parsed, daemon check at startup + 24 h interval, silent on failure, consent prompt with
notes and size, stop → update → relaunch → restore running services, skip/later persisted."*

The feature document is [updates.md](../../../.claude/features/updates.md), written when the updater
was Tauri v2's and kept when that left with
[ADR 0011](../../../.claude/decisions/0011-no-gui-in-this-repository.md).

**Three things this task changes about the sentence it was written from**, each argued below:
the order is **download → verify → stop → swap** and not *stop → download* (D5); the artifact is
bound to the feed by **SHA-256 inside a minisign-signed document** rather than by a second detached
signature (D3); and the whole sequence is driven **by the daemon** rather than by `mix`, because
`mix` may not depend on `mixengine-core` (D4).

## Second pass — 2026-09-05

This document was written on 2026-09-04, before T86, T87 and T94 landed. Read again against the tree
those three left behind, it was wrong or silent in eight places, and each of them is corrected in the
decision it belongs to rather than listed here and forgotten:

| | What the first pass said | What it says now |
| --- | --- | --- |
| **D1** | `Document::FRESH_FOR` becomes an associated constant | dropped — it can only change how often a *restart* re-fetches, and six hours is already that answer (D1) |
| **D3** | the feed carries `url`/`sha256`/`size` per platform | the feed's artifact **is** [`index::format::Artifact`], so `provides` rides along and the whole of `core::install` is reused rather than re-implemented (D3) |
| **D6** | every packaging script gains a plain archive | …and all three hold **one top-level `mixengine/` directory**, which is what makes one `provides` shape describe six artifacts (D6) |
| **D6** | — | macOS is *universal* and the feed is keyed by `(os, arch)`: the `.pkg` leg emits **two rows for one tarball** (D6) |
| **D8** | — | a swap that rolls back must also **start again what the stop stopped**, or a failed update leaves a developer's database down (D11) |
| **D10** | the new daemon reads `updates.applied` and `updates.restore` | …and **deletes them before acting on them**, or every later start replays a restore the user has since undone (D10) |
| **notes** | `latest.json` carries `notes` | the feed is signed in CI *before* the draft release exists, so `--generate-notes` cannot reach it: notes come from `git log` and a `notes_url` points at the page a person edits afterwards (D13) |
| **D7** | the placement probe writes a file and removes it | …under a name, and removes a stale one a crash left (D7) |

Two more readings this pass took that changed nothing, and are written down so the next reader does
not re-take them:

- **`update.apply` stays one blocking call rather than a job.** `mix`'s HTTP client sets no request
  timeout — `crates/mixengine-cli/src/client.rs` builds a `hyper` connection and nothing else — so a
  two-minute call cannot fail for being long. And a job whose completion *is* the daemon exiting is a
  job nothing can ever observe finishing, which is a worse shape than a call with no progress bar.
  What a 15 MB fetch needs is the size printed before it starts, which is what the consent prompt
  already prints.
- **`mixengine-shim` is in no artifact this project ships.** `packaging/stage.sh` builds
  `-p mixengine-cli -p mixengine-daemon -p mixengine-elevate` and copies `MIX_BINARIES`, which is
  three names; `core::shims::source` looks for a fourth beside the running `mixengined` and raises
  `Error::ShimMissing` when it is not there. That is every runtime command the product exists to
  provide, on every installed copy. It is **T85's defect and not this task's**, and the first pass
  buried it in a paragraph of D6 — so it is promoted to roadmap task **T85c**, placed before T88 in
  phase 9. This task is written so that the fix is adding the name and nothing else: the swap set is
  the payload's contents intersected with what is installed (D11).

[`index::format::Artifact`]: ../../../crates/mixengine-core/src/index/format.rs

## Goal

A person running MixEngine 0.1.0 sees, in `mix status`, that 0.2.0 exists. They type
`mix self-update`, read the notes and the size, answer *install*, and a minute later they are
running 0.2.0 with the same services up that were up before. A person who answers *skip* or *later*
is not asked again. A person with no network sees nothing at all, and their daemon starts no slower
for it. A person who installed from a `.deb` is told to update with `apt` rather than being handed a
permission error.

## Scope

**In:**

- `mixengine-core::updates`: the `latest.json` document and its verified client, where a release's
  binaries are placed and whether this account may replace them, the apply sequence, and the
  skip/later/restore records in the `settings` table.
- `mixengined`: a check at startup and on a 24 h clock, both silent on failure; the four
  `update.*` methods; restoring services and cleaning up `.old` files after an update's restart.
- `mixengine-proto`: `UpdateStatus`, `UpdateRelease`, `UpdatePlacement`, `UpdateDecision`,
  `UpdateApplied`, `DaemonEvent::UpdateAvailable`, and an **optional** `DaemonStatus::update`.
- `mix self-update`, `mix self-update --check`, `--yes`, and the update line in `mix status`.
- `[updates]` in `config.toml`, and `--update-url` / `--update-key` on `mixengined`.
- The **update payload**: a plain archive of the release's binaries, produced by every packaging
  script beside the installers that already exist, all three holding one `mixengine/` directory (D6).
- `packaging/feed.sh`, which writes `latest.json` into the distribution directory before
  `packaging/sign.sh` signs it, and the `release` job step that runs it (D13). **Not** the key:
  T86 generated it and `updates::PUBLIC_KEY` already pins it.
- Documentation: `updates.md`, `build-and-release.md`, `packaging/README.md`, the roadmap.

**Out:**

- **T88a** — the `mixengine-elevate` update path. This task *excludes* the helper from the swap by
  name and reports it as kept; replacing it needs its own elevation prompt, a minisign check inside
  the elevated context, and daemon↔elevate protocol negotiation, none of which is here.
- **T86** — the updater key, the Actions secrets, and the signing of release artifacts in CI.
  **Done** (2026-09-04): the key is generated, `core::updates::PUBLIC_KEY` is pinned, and
  `packaging/sign.sh` signs every artifact a release carries, from a `release` job that assembles a
  draft. What is left for this task is `latest.json` itself and the payload archives it lists (D2,
  D6).
- **T88c** — the `DaemonStatus` skew rule. This task adds a field to that struct and deliberately
  does not add to that debt: see D9.
- **T89** — the migration test. Nothing here changes a schema; the `settings` table has existed
  since `0001_initial.sql` and this is its first user.

## Decisions

### D1 — `latest.json` is a third `index::Document`, not a second client

`core::index::Client<D: Document>` already does, and already has tests for, the three properties
this feed needs:

- **the signature is checked before the bytes are parsed**, so a JSON parser never runs on
  unverified input;
- **the cache is re-verified on every read**, because a file in the user's home is a file any local
  process can rewrite;
- **a correctly signed document from before the one we hold is refused**, because every version we
  ever published verifies against the same key and only `generated_at` separates them.

A rolled-back update feed is exactly the attack that matters here — an attacker who can answer the
URL replays yesterday's feed to keep a machine on a version with a known hole — so getting the third
property for free is the whole argument.

**One change to the generic client**, which the index keeps its current behaviour under:
`Client::refresh()` is made public. It is `catalogue()` without the cache shortcut, and
`catalogue()` becomes *"if the cache is fresh, that; otherwise `refresh()`"* — which is what it
already was, spelled as two functions instead of one.

That gives the three callers this task needs exactly one function each: the startup check uses
`catalogue()` so that a daemon restarted ten times in an hour makes one request; the 24 h tick uses
`refresh()`, because the clock *is* the policy; and `update.check` from `mix self-update --check`
uses `refresh()`, because the feature document says that command forces an immediate check.

**The first pass also proposed a per-document `FRESH_FOR`, and this pass drops it.** The only call
that reads it is `catalogue()`, and the only caller of `catalogue()` here is the startup check — so
the constant can change one thing: how often a daemon that is restarted repeatedly goes back to the
network. Six hours is already the right answer to that, for the reason `index::FRESH_FOR` states
about a security release. A knob whose every setting produces the same behaviour is a knob three
documents would have to carry for nothing.

The document — one `schema`, one `generated_at`, and an `artifacts` list whose entries are
[`Artifact`](../../../crates/mixengine-core/src/index/format.rs) itself (D3):

```json
{
  "schema": 1,
  "generated_at": "2026-09-04T09:12:00Z",
  "version": "0.2.0",
  "published_at": "2026-09-04T09:12:00Z",
  "notes": "fix(dns): answer a wildcard under a two-label TLD\nfeat(cli): mix self-update",
  "notes_url": "https://github.com/mixnz/mixengine/releases/tag/v0.2.0",
  "artifacts": [
    { "os": "windows", "arch": "x86_64",
      "url": "https://github.com/mixnz/mixengine/releases/download/v0.2.0/mixengine-0.2.0-windows-x86_64.zip",
      "sha256": "…", "size": 14680064,
      "provides": { "mix": "mixengine/mix.exe",
                    "mixengined": "mixengine/mixengined.exe",
                    "mixengine-elevate": "mixengine/mixengine-elevate.exe" } }
  ]
}
```

Both moments are `index::format::Timestamp` — the strict `YYYY-MM-DDTHH:MM:SSZ` the packaging
pipeline's `date -u` already writes and the index client already parses — and `published_at` reaches
the wire as that string, for the same reason `DaemonStatus`'s paths do: it is for reading, and
nothing joins or subtracts it.

`generated_at` and `version` are two fields and not one on purpose. `version` is what is offered;
`generated_at` is what makes a replay detectable, and a feed re-published for the same version — a
corrected note, an added architecture — must be able to move forward without pretending to be a new
release.

`schema` is checked against a compiled-in `SCHEMA` before anything else is believed, and unknown
fields are ignored, both for the reasons `index/format.rs` states: this document is written by us
and read by builds older than it.

### D2 — the updater key is a third key, generated by this task

`~/.config/mixengine/` already holds two private keys: `minisign.key` signs the package index,
`blueprints.key` signs the blueprint gallery. This adds `updates.key`, and pins its public half as
`core::updates::PUBLIC_KEY`.

**A third key rather than reusing the index's**, on the gallery key's own argument: the two are
published from different repositories, by different workflows, with different Actions secrets, and a
key that signs "what MixEngine will execute as your `mixengined`" should not be the key that a
compromise of the packaging repository hands an attacker.

**Generated by T86 after all.** This paragraph used to say the opposite — that the key was generated
here because a constant that is not a valid minisign key makes every test in this task untestable and
the production path unbuildable. That argument is sound and the roadmap order answered it the other
way: T86 comes first, generation is its own sentence, and it landed on 2026-09-04 with
`core::updates::PUBLIC_KEY`, `packaging/updates.pub`, `packaging/sign.sh` and a `release` job already
in place. What this task therefore takes from T86 is the whole key *and* the signing pipeline; what
T86 left here is `latest.json` itself, which lists the payload archives D6 creates and could not have
been written before them. Because the release job signs whatever is in the distribution directory,
writing the feed into that directory is the whole of the change.
See [the T86 design](2026-09-04-t86-updater-signing-design.md).

Overriding the pair is the index's mechanism, verbatim: `--update-url` requires `--update-key`, both
readable from `MIXENGINE_UPDATE_URL` / `MIXENGINE_UPDATE_KEY`, and neither is read below `main`. A
URL that could move while the key could not would be a setting that can only ever fail.

### D3 — the artifact is bound to the feed by SHA-256, and that *is* the minisign check

`updates.md` says *"download → verify minisign → install"*, and the acceptance criterion is *"a
tampered artifact fails the minisign check and is refused"*. Both are satisfied by hashing, and the
mechanism is the one `core::index` already uses for every runtime this product installs:

> the signature covers the document, the document carries the artifact's SHA-256, so a tampered
> artifact fails a check whose root of trust is the Ed25519 signature.

A second, detached `.minisig` beside the payload would add a second key handling path, a second
fetch, and a second failure mode, to establish a property the first one already establishes. The
feature document is amended to say *how* rather than to imply a mechanism it did not mean to
mandate.

**And the artifact entry is [`index::format::Artifact`] itself rather than a type of its own** —
this pass's change, and the one that decides how much code this task writes. The first pass listed
four fields (`os`, `arch`, `url`, `sha256`, `size`) that are the first five of that struct's nine,
and would then have needed its own downloader to use them. Taking the whole struct instead means the
feed also carries `provides`, and `provides` is the key to `core::install`:

| What the updater needs | What `Installer::install` already does with an `Artifact` |
| --- | --- |
| download, resumable across a restart | the `.part` file named after the hash, in `cache/downloads/` |
| the payload is the one the signed feed named | `verify` against `artifact.sha256` → `Error::ArtifactChecksum` |
| unpack without letting an entry escape | `archive::extract` → `Error::UnsafeArchiveEntry` |
| the payload holds the binaries it claimed | `present` over `artifact.provides` → `Error::MissingFromArtifact` |
| the staged `mixengined` runs *here* (D8) | `smoke` with a [`SmokeTest`] → `Error::SmokeTestFailed` |
| a half-unpacked payload never appears | the staging directory, discarded on any failure, renamed on success |

So steps 3a–3d of D4's table are one call — `Installer::install(&artifact, cache/updates/<version>,
Some(&SmokeTest { executable: "mixengined", args: ["--version"] }), NotAnArchive::Refuse, watcher)` —
and this task writes no download code, no checksum code, no unpacking code and no smoke-test code. It
writes the swap, which is the only part of an update that is not an install.

`requires` rides along for free and is the reason not to trim the struct down: a Linux payload past
this machine's glibc floor is a fact the packaging pipeline already measures, and a feed that could
carry it is a refusal that happens before a byte is downloaded rather than at the smoke test.

[`SmokeTest`]: ../../../crates/mixengine-core/src/install.rs

**This does not extend to T88a**, and the distinction is worth writing down. `mixengine-elevate` is
replaced inside an elevated context by a process that did not fetch the feed and must not trust the
daemon that did — `updates.md`'s single most important rule. That path needs a signature it can
check itself, against a key pinned in the copy already installed. Publishing a detached signature
for the helper is T88a's, and it does not make one necessary here.

### D4 — the daemon does the work; `mix` prompts and reconnects

`mixengine-cli` may depend on `mixengine-platform` and `mixengine-proto` and on nothing else —
`workspace_layering.rs` enforces it, and the comment there gives the reason: `mixengine-core` carries
`sqlx`, and *"linking a bundled SQLite into `mix` to learn that `run/` sits under the root is a
trade nobody would make"*. Verifying a signature, unpacking an archive and swapping files are all
`core`'s, so all of them happen inside `mixengined`.

That reads, at first, like a contradiction of `updates.md`'s *"`mix self-update` is therefore the
one command that outlives the daemon it is updating"*. It is not. What has to outlive the daemon is
the *client*, and what it has to do afterwards is exactly one thing — start the new one — which is
`Autostart::run()`, a mechanism `mix` has had since T9.

The sequence, with the process that performs each step:

| # | Step | Who |
| --- | --- | --- |
| 1 | `update.status` → version, notes, size, what will be restarted | daemon |
| 2 | prompt: *install / skip this version / remind me later* | `mix` |
| 3 | `update.apply` | |
| 3a | download the payload, resume-capable, into `cache/updates/` | daemon |
| 3b | SHA-256 against the signed feed | daemon |
| 3c | unpack into a staging directory, refusing unsafe entries | daemon |
| 3d | **run the staged `mixengined --version`** — the smoke test | daemon |
| 3e | stop every supervised service in reverse dependency order | daemon |
| 3f | record what was stopped, and what version this update is going to | daemon |
| 3g | swap the binaries, `mixengine-elevate` excluded by name | daemon |
| 3h | answer the call, then exit | daemon |
| 4 | wait for the endpoint to stop answering | `mix` |
| 5 | `Autostart::run()` — start the new daemon and return when it listens | `mix` |
| 6 | start the services that were stopped; clean up `.old`; check the version | new daemon |

Step 3h is `daemon.shutdown`'s mechanism unchanged: the `Going` guard is taken, the answer is
encoded by the caller, and the cancellation token drops after the walk and before the connection
closes. A client sees the answer and then the connection close, which *is* the update rather than a
failure of it.

Step 5 deliberately does **not** open a `Client`. The new daemon may speak a newer protocol than the
`mix` that is still running from the old image, and a protocol-mismatch error at the end of a
successful update would be the worst possible last line. `--detach` exiting zero is the readiness
probe, exactly as `autostart.rs` already documents.

### D5 — download before stopping, not after

`updates.md` lists *"stop supervised services in reverse dependency order → download → verify
minisign → install"*. Taken literally that leaves a developer's database down for the length of a
download, on a connection nobody promised anything about, in order to gain nothing: a download that
fails after the stop has cost an outage, and a download that succeeds could have happened while
everything was still up.

The order becomes download → verify → unpack → smoke → **stop** → swap → relaunch → restore. The
window in which services are down is the swap and the restart, which is seconds. `updates.md` is
amended.

### D6 — the payload is a plain archive of binaries, and packaging must produce one

The five artifacts packaging builds today are all *installers*: an NSIS setup, a `.pkg`, a `.deb`,
an `.rpm`, an AppImage. None of them is a thing an updater can apply — three need root, one needs a
GUI dialog on macOS 15, and the AppImage is a file the user placed, not a directory of binaries.

So every packaging script gains one more output beside what it already makes: a plain
`mixengine-<version>-<os>-<arch>.(zip|tar.gz)` holding the release's binaries and nothing else.
Windows already produces exactly this — the portable zip — and this makes the other two rows match
it. The updater applies that, and never runs an installer.

**All three hold one top-level `mixengine/` directory**, which is this pass's addition and is what
makes D3 work. `packaging/windows/build.sh` already writes its zip that way (`Compress-Archive` over
`$MIX_OUT/zip/mixengine`); the two new tarballs are written to match rather than the zip being
flattened to match them, because a zip a person extracts into `Downloads` should not scatter three
binaries there. One layout for six artifacts is one `provides` shape for the feed generator to
compute and one path for the updater to read.

**macOS is universal, and the feed is keyed by `(os, arch)`.** The `.pkg` leg builds both slices and
`lipo`s them into one binary, so there is one tarball and there are two architectures that can
install it. The feed therefore carries **two rows pointing at the same URL**, one per arch, rather
than an `arch: "universal"` that every reader would have to special-case — `Arch` is a closed enum of
two variants for exactly the reason a third spelling is a bad idea, and a client asking "is there a
build for this machine" should get the answer by matching the pair it already has.

**Which binaries a release is made of is one list**, `MIX_BINARIES` in `packaging/common.sh`, read
by the packaging scripts. The updater does **not** mirror it: `core::updates` iterates the *payload's
own* `provides` map, intersected with what is present in the install directory (D11). A list
compiled into the binary would be a fourth copy of the same three names and would be the wrong copy
on exactly the release that changes it — the day `mixengine-shim` is added (T85c), an installed 0.2.0
must be able to take a 0.3.0 payload that has one.

### D7 — a release this account cannot write is refused, in words

`core::updates::Placement::of(&daemon_exe)` answers one of two things:

- `SelfUpdatable { directory }` — a probe file was created and removed in the directory holding the
  daemon.
- `Managed { directory, because }` — it could not be, or `APPIMAGE` is set in the environment.

That covers the four ways of installing that this task must refuse without a stack trace: `/usr/bin`
from a `.deb` or an `.rpm`, `/usr/local/bin` from a `.pkg`, and a read-only AppImage mount. It is a
**write probe and not a path table**: a list of "system" prefixes would be per-OS knowledge in
`core`, which `CLAUDE.md` forbids, and would be wrong for anybody who installed somewhere unusual.
The probe asks the only question that matters and asks it of the actual machine.

**The probe file has a name, and a stale one is removed rather than believed** — this pass's
correction to a sentence that said "a probe file" and left it there. It is
`.mixengine-update-probe`, it is removed in the same function that creates it, and a copy left by a
daemon that was killed between the two is deleted on the next probe instead of making the directory
look occupied. A dotted, product-named file rather than a random one, because the failure mode worth
designing for is somebody finding it in `%LOCALAPPDATA%\Programs\MixEngine` and wondering what wrote
it.

`Managed`'s `because` is a sentence and never a package-manager command: which of `apt`, `dnf`,
`brew` or an AppImage put a binary in a directory is per-OS knowledge, and `core` may not hold it.
What it can say honestly is *"this copy of MixEngine is in a directory this account cannot write
(`/usr/bin`), so it was installed by something else and that is what updates it"*, which is the same
information without a guess in it.

`update.status` carries the placement so a client can render it before anybody commits to anything,
and `update.apply` refuses with `PreconditionFailed` when it is `Managed`. Neither ever attempts an
elevation: an updater that could ask for root would be the privilege-escalation path this whole
feature is written to avoid.

### D8 — the smoke test runs before the swap, and the version check after the restart

**Before the swap**: the staged `mixengined --version` is executed. This is `core::install`'s
`SmokeTest`, which every runtime install already uses, and it is the only thing that can catch — in
time to do nothing — a payload for the wrong architecture, a Linux build past this machine's glibc
floor, and **a Windows Code Integrity refusal**. `updates.md` records that Smart App Control judges
each file separately, again after every update, with refusal rather than a warning at the end of it;
a smoke test is the difference between "the update was refused, nothing changed" and "MixEngine no
longer starts".

**After the restart**: the new daemon compares its own `CARGO_PKG_VERSION` with the `to` field of
the record the old one wrote. This is where the check belongs, because it is the first moment
anything can answer it honestly — the running binary's own version — and because putting it before
the swap would mean trusting a `--version` string from a payload to describe the payload.

If they differ, the daemon writes `updates.skipped_version = <to>` and logs a warning. A
mislabelled release therefore costs one pointless update and then stops being offered, instead of
being offered forever by a daemon that reinstalls it every 24 h.

### D9 — `DaemonStatus.update` is optional, and does not add to T88c's debt

**T88c** records that `DaemonStatus` is not backwards compatible within one protocol version,
because `elevation` (T40b) and `dns` (T44) are both **required** fields added after protocol 1 was
frozen — so a new `mix` cannot deserialise an old daemon's answer, and the note `render::status`
carries for exactly that skew is unreachable.

This task adds a third field and must not make that worse. It does not, and not by exemption:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub update: Option<UpdateOffer>,
```

`None` is the honest value for a daemon that has not checked yet, for one whose check found nothing,
and for one built before the field existed — three states a client renders identically. So the field
is optional because of what it means, and the skew-tolerance is a consequence rather than a
workaround. T88c's decision about the other two fields is unaffected either way.

### D10 — skip, later and restore live in the `settings` table

`settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL) STRICT` has existed since
`0001_initial.sql` and has never had a row in it. This is its first user, and it needs no migration.

| Key | Value | Written by | Read by |
| --- | --- | --- | --- |
| `updates.skipped_version` | `"0.2.0"` | `update.decide`, and D8's post-restart check | the offer decision |
| `updates.remind_after` | a `Timestamp` | `update.decide` | the offer decision |
| `updates.applied` | `{ "from": …, "to": …, "at": … }` | 3f | the new daemon at start |
| `updates.restore` | `["php-8.3", "mariadb-11"]` | 3f | the new daemon at start |

`remind_after` is a wall-clock time, and a machine whose clock was a year fast when somebody answered
*later* holds a moment a year away once the clock is corrected — and would then never be offered
anything again.

**What that gets is not a clamp, and the implementation found it.** The first draft of this
paragraph said the value is *"clamped on read to at most seven days ahead"*, and the test written
from that sentence failed: `min(stored, now + 7 days)` is re-evaluated on every read, so the
deadline moves forward each time it is read and never comes due at all. The rule is therefore
**ignore, not clamp** — a stored moment more than seven days ahead is not a reminder anybody asked
for, so it is disbelieved and the release is offered. `remind me later` writes `now + 3 days`, which
is well inside that, so the two constants are related and a test asserts the relation rather than
leaving it to be re-derived: three days against a daily check, because one day is tomorrow and a
week is long enough that a security release waits behind a shrug.

**Both records are deleted before they are acted on, and that is not a tidiness rule.** This pass's
correction: the first said the new daemon *reads* them and said nothing about removing them. A
`updates.restore` that survives being read is replayed by every later start — so a person who
updates, stops MariaDB because they are done with it, and restarts their machine gets MariaDB back,
for ever, with nothing in the product able to tell them why. Deleted *before* rather than after, so
that a start which crashes half way through the restore does not make the record immortal either:
the cost of the delete-first order is one lost restore on a daemon that died mid-start, and the cost
of the other order is a home that can never stop a service again.

**Restoring is the daemon's, not the client's.** `updates.restore` is the `reached` list of the
`ServiceWalk` the stop produced — the daemon's own answer to "what was running" — and the new daemon
starts them in the reverse of that order, dependencies first. A client that read a list and issued
`service.start` calls would be deciding an order, which is the business-logic-in-a-client bug
`CLAUDE.md` forbids. The pass is spawned rather than awaited, on the same reasoning the extension
configuration block already gives: the endpoint is bound, and every moment spent before `accept` is
a moment a second client on Windows meets `ERROR_PIPE_BUSY`.

### D11 — the swap is rename-then-write, and rolls itself back

For each name in the payload's `provides`, excluding `mixengine-elevate`:

1. If nothing of that name exists in the install directory, record it as absent and skip. This is
   how a payload that gains a binary behaves against an install that does not have it yet, and how
   `mixengine-shim` will behave the day T85c is done.
2. `rename(target, target.old)`.
3. Copy the staged file to `target`, and set mode `0o755` where the platform has modes — a `.zip`
   does not carry the executable bit, and `mix` that cannot be executed is not an update.

Any failure renames every `.old` back before returning, so a partial swap is never left behind.

**And the rollback starts again what the stop stopped** — this pass's addition, and the hole the
first pass left open. By the time a swap can fail, D5's order has already stopped every supervised
service; a rollback that put three files back and returned an error would leave a developer's
database down, with the update refused and nothing on the machine intending to start it again. So
the apply sequence's failure path is: rename the `.old` files back, start the services in
`updates.restore`'s reverse order — the same pass the new daemon would have made — clear both
records, and *then* return the error. The daemon does not exit: it is still the daemon it was before
the attempt, running the binaries it was running, and there is nothing for `mix` to relaunch.

That is also why the record is written at 3f, *before* the swap, rather than after it: it is read by
whichever of the two paths happens, and one of them is inside this same process.

Renaming rather than overwriting is what makes this work at all on Windows, where the running
`mix.exe` is one of the files being replaced: an open image cannot be deleted or written, and it
*can* be renamed, after which the freed name accepts the new file. On Unix a rename over a running
binary would also be safe, and doing it the same way on both keeps one code path.

`.old` files are removed by the **next daemon start that succeeds**, which gives the property that
matters for free: they survive exactly as long as they are the only way back, and a daemon that
comes up has proved they are not needed. A `mix.exe.old` still held open by the `mix` that ran the
update is left for the start after that.

### D12 — one `mix self-update` at a time

`mix` takes an exclusive lock on `<home>/run/self-update.lock` — `platform::lock`, the mechanism the
daemon already uses for its own single-instance guarantee — for the whole of steps 2 to 5. Two
updates racing would otherwise interleave a swap with a relaunch, and the second would find `.old`
files written by the first.

**A client that goes away mid-apply does not stop the apply**, which is `daemon_shutdown`'s rule
one method along and for its reason: the first thing the handler does past the smoke test is stop
the services, and a Ctrl-C that abandoned the work between the stop and the swap would leave a home
with everything down and half its binaries renamed. Before that point there is nothing to protect —
an abandoned download leaves a `.part` file in `cache/downloads/`, which is exactly what the next
attempt resumes from.

### D13 — the feed is written in the `release` job, and its notes come from `git`

`packaging/sign.sh` signs every file in `$MIX_OUT/dist` that is not a `.sha256` or a `.minisig`. So
`latest.json` is signed by writing it into that directory before that step, and `latest.json.minisig`
— the name `index::Client` appends — comes out of it. That much D2 already said.

What it did not say is **where the notes come from, and there is only one answer that works.** The
`release` job's order is: gather the five legs → sign → `gh release create --draft --generate-notes`
→ upload. The notes GitHub generates therefore do not exist until after the signing is over, and a
document signed before them cannot contain them. Re-signing afterwards would put the private key on
the machine of whoever edits the draft, which is the one thing T86 arranged not to need.

So `packaging/feed.sh` writes the notes itself, from `git log <previous tag>..<this tag>` — the same
commit subjects `--generate-notes` starts from, taken from the repository the job has already checked
out. And the document carries a **`notes_url`** beside them, pointing at the release page, so a
person who edited the draft into something better has somewhere to send a reader. `mix self-update`
prints the notes and then the URL.

The generator is otherwise a directory listing: for each payload archive in `dist` — the names D6
fixes, and nothing else in there — it reads the size, reads the `.sha256` beside it, opens the
archive to build `provides`, and emits a row per `(os, arch)` with macOS emitting two (D6). Opening
the archive rather than assuming its layout is deliberate, on `build.sh`'s own rule: *"an empty
archive is a perfectly valid archive, and this is the only step that would notice."*

`generated_at` and `published_at` are `date -u +%Y-%m-%dT%H:%M:%SZ`, which is the strict spelling
`index::format::Timestamp` parses and the only one it does.

## Wire surface

```
update.status                          → UpdateStatus     no network
update.check   { force: bool }         → UpdateStatus     goes to the network
update.decide  { version, decision }   → UpdateStatus     skip | later
update.apply   { version }             → UpdateApplied    and then the daemon exits
```

`update.apply` takes the version the client showed the user, and the daemon refuses if that is no
longer what the feed offers. Without it, a check that lands between the prompt and the answer would
install something the user never read the notes for.

```rust
pub struct UpdateStatus {
    pub current: String,
    pub available: Option<UpdateRelease>,
    pub offered: bool,
    pub because: Option<String>,
    pub checked_at: Option<Timestamp>,
    pub stale: bool,
    pub placement: UpdatePlacement,
    pub will_restart: Vec<ServiceId>,
}

pub struct UpdateApplied {
    pub from: String,
    pub to: String,
    pub directory: String,
    pub replaced: Vec<String>,
    pub kept: Vec<String>,
    pub restarting: Vec<ServiceId>,
}
```

`stale` is this pass's addition and is `Freshness::is_stale` passed through rather than re-derived:
`index::Client` answers from its cache when the network refused, so an offer can perfectly well be
made from a document read three days ago. That is a genuine offer and not an error — the signature
was checked exactly as it would have been on a fresh copy — but *"checked 3 days ago"* is a different
sentence from *"checked just now"*, and a client that had to work out which from `checked_at` and its
own clock would be deriving what the daemon already knows.

`UpdateApplied` is what `mix` has left to work with once the daemon is gone. `replaced` and `kept`
are the swap's own answer — `kept` is `mixengine-elevate` by name (T88a), and anything the payload
carried that this install does not have. `directory` and `replaced` together are what step 5's
failure message prints: the `.old` paths and the command that puts them back.

`offered` is the daemon's decision and `because` is its reason — *"you skipped this version"*,
*"you asked to be reminded on the 11th"*, *"this release has no build for windows/aarch64"*. A
client renders the sentence and does not re-derive it. `will_restart` is what makes the consent
prompt able to say *"3 services will be stopped and started again"*, which is `updates.md`'s
*"never update while a supervised service is under load without asking"* in the only form that rule
can take once consent is always required.

`DaemonEvent::UpdateAvailable { version, published_at }` is published **once per version**, using
`certs::renewal`'s `newly` rule: a producer reports a change and not a heartbeat, and a check that
runs every 24 h for a month must not spend a client's stream allowance restating one fact.

## CLI

```
mix self-update            check, show version + size + notes, prompt, apply
mix self-update --check    check and print; never applies
mix self-update --yes      no prompt; for scripts and for a machine with no terminal
```

The prompt has three answers and mirrors `confirm::Choice`, which T78 added for exactly this shape
of question: *install / skip this version / remind me later*. End of file is neither — a script that
could not be asked is told which flag says yes in advance, which is what `Choice::NobodyThere`
already exists to say.

`mix status` gains one line when an update is offered, from `DaemonStatus::update`, and prints
nothing at all when it is not.

## Configuration

```toml
[updates]
enabled = true          # a machine that must never check can say so
check_seconds = 86400
```

`enabled = false` turns off the startup check, the clock, and the event — and leaves
`mix self-update --check` working, because a person who typed the command is asking.

## What can go wrong, and what each thing does about it

| Failure | Where it is caught | What the user gets |
| --- | --- | --- |
| no network, at startup or on the clock | the check | nothing at all, and a `debug!` line |
| no network, at `mix self-update` | the check | the last verified feed, marked stale, or the transport error |
| the feed does not verify | `Error::IndexSignature` | the cached feed is kept and the refusal is logged — `Client`'s existing fallback |
| the feed is older than the cached one | `Error::IndexRolledBack` | the same |
| the release has no build for this OS and architecture | the offer decision | `offered: false`, with `because` naming the pair |
| the payload's hash is not the feed's | `Error::ArtifactChecksum` | the apply fails, nothing is swapped |
| the payload contains a path that escapes its root | `Error::UnsafeArchiveEntry` | the same |
| the payload is missing a binary it declared | `Error::MissingFromArtifact` | the same |
| the staged `mixengined` will not run here | `Error::SmokeTestFailed` | the same, and this is the Code Integrity case |
| the install directory is not writable | `Error::UpdateNotWritable` | a refusal naming the directory, before anything is downloaded |
| the swap fails part way | D11's rollback | every `.old` renamed back, **the stopped services started again**, and the failure returned |
| the new daemon does not come up | `mix`, at step 5 | the paths of the `.old` files and the one command that puts them back |

Three new error variants, and no more: `Error::UpdateNotWritable { directory }`,
`Error::UpdateNotOffered { asked, offered }` for a version the feed no longer names, and
`Error::UpdateUnavailable { os, arch }`. Everything else in that table is an error that already
exists, raised by the code that already raises it — which is the point of D1 and of reusing
`core::install`.

**Step 4's wait is bounded.** `mix` polls the endpoint until connecting to it says *absent*, for at
most thirty seconds. A daemon that has answered `update.apply` has already stopped its services and
cancelled its token, so anything longer than that is a supervised process refusing to die — which
`daemon.shutdown` already reports through `ServiceWalk::failed` and which does not stop the
relaunch. Past the timeout `mix` starts the new daemon anyway: the endpoint is a socket the new one
will rebind or a pipe it will re-create, and a user left with no daemon is worse than a second one
failing to start and saying so.

**Step 5 failing is the one path that leaves a machine worse than it found it**, and it is why D8's
smoke test exists at all — after it, a `mixengined` that does not start is a binary that ran
`--version` successfully minutes earlier. `mix` reports the two `.old` paths and the copy command
that undoes the swap, and exits non-zero. It does not attempt the rollback itself: the files are the
daemon's to place, `mix` may not link `mixengine-core`, and a client that moved binaries around on a
failure it does not understand is a client doing something to the machine.

## Testing

| What | Where | How |
| --- | --- | --- |
| the feed verifies before it parses; a bad signature is refused | `core` unit | testkit's signing fixture |
| a feed from before the cached one is refused | `core` unit | two documents, one key |
| a version not newer than the running build is not offered | `core` unit | `PackageVersion::cmp_precedence` |
| skip and later suppress the offer; later expires | `core` unit | a store and a clock |
| a clock corrected forward does not suppress a reminder for a year | `core` unit | `remind_after` clamped on read |
| the placement probe refuses a directory this account cannot write | `core` unit | a read-only directory, and `APPIMAGE` |
| a swap that fails half way puts everything back | `core` unit | a staging directory missing its second file |
| a record that was read is gone from the store | `core` unit | restore twice, and the second pass finds nothing |
| `mixengine-elevate` is never in the swap set | `core` unit | a payload containing it |
| a payload with a name this install does not have leaves it alone | `core` unit | a `provides` naming four binaries against a directory holding three |
| the feed's notes and `provides` describe the archives beside it | `packaging` | `feed.sh` over a fixture directory, in the `lint` job beside `test-sign.sh` |
| a payload whose `mixengined` will not run is refused, nothing is swapped | `cli` integration | a stub that exits 1 |
| the whole sequence: stop, swap, relaunch, restore | `cli` integration | a served feed and a payload built from the test's own binaries |
| a payload whose version is not the one offered is not offered again | `cli` integration | falls out of the above, since the payload's version *is* the running one |

The end-to-end test copies `mix` and `mixengined` into a temporary install directory, runs the
daemon from there against a served feed and payload, and asserts: the files changed, the
`mixengine-elevate` copy did not, the services that were running are running again, and the new
daemon recorded the version mismatch rather than offering the same release forever. That last one is
D8's post-restart check being exercised by the only test that can exercise it — a test cannot build
a binary with a different `CARGO_PKG_VERSION`, and the check is written where the payload being the
same version is a *pass* of the test rather than a hole in it.

Nothing in this feature is `#[ignore]`d: the network is a served fixture, the swap is a temporary
directory, and no part of it touches anything outside `MIXENGINE_HOME` — which is the property that
makes an updater testable at all, and the reason the elevate helper is excluded rather than
special-cased.

## Documentation changed

- `.claude/features/updates.md` — the order (D5), the artifact's chain of trust (D3), the placement
  refusal (D7), and what the smoke test is for (D8).
- `.claude/operations/build-and-release.md` — the update payload beside the installers, `feed.sh` in
  the `release` job, and which half of the signing row is now built.
- `packaging/README.md` — the sixth artifact per OS, and the feed.
- `.claude/roadmap/phase-9-ship.md` — T88 ticked, and **T85c added** for the missing
  `mixengine-shim` (the second pass, above).
