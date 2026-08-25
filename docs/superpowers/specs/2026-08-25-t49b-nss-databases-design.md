# T49b — the databases the browsers read instead, and the tool that is not installed

Firefox and Chrome on Linux do not read `/etc/ssl/certs`. They carry their own certificate
databases, one per profile, in NSS format, and a machine whose system trust store holds MixEngine's
authority still shows a red padlock in both browsers. T49a made the system store honest; this task
is the half of `.claude/features/tls.md`'s table that was split off at the privilege boundary,
because these databases belong to the user and no prompt is involved in writing them.

It starts from two measurements that the specification does not have.

**On a stock Ubuntu 24.04, `certutil` is not installed.** It ships in `libnss3-tools`, which is not
pulled in by `libnss3` and not present in a default install. `tls.md` names the command as if it
were always there. A machine without it is a state to report.

**`tls.md`'s Firefox path is wrong for the distribution most people run.** `~/.mozilla/firefox/*/`
is where a deb or tarball Firefox keeps profiles, and on Ubuntu 22.04 and later the `firefox` deb is
a transitional package: `Version: 1:1snap1-0ubuntu5`, `Description-en: Transitional package -
firefox -> firefox snap`, `Pre-Depends: snapd (>= 2.54)`. The snap's profiles are at
`~/snap/firefox/common/.mozilla/firefox/*/`, which the table does not mention, so a faithful
implementation of the table would find nothing on the default Ubuntu desktop and report success.

---

## D1. A trait of its own, and the reason is arity

`TrustStore` answers one question about one store. NSS is **N databases**, and they are
**orthogonal** to the system store: a machine can hold the authority in `/etc/ssl/certs` and in
none of its browsers, or in a Firefox profile and not in the system store, and both are ordinary
states rather than contradictions.

Adding `TrustStoreMethod::Nss` would make `TrustState.installed` — a `bool` — answer for a set. The
honest answers are "in three of five", "in all of them but the tool is missing so nothing was
checked", "there are no databases here". None of those fit a `bool` and a `String`, and squeezing
them in is the shape collision T48 made once already, where one `because` had to carry two
different reasons.

So: a second trait, `BrowserTrust`, in `crates/mixengine-platform/src/traits/browsers.rs`, beside
`trust.rs`. Both are accessors on `Host`. Neither knows about the other.

## D2. Linux is where MixEngine searches, and the other two say that rather than claim anything

The search runs on Linux. Windows and macOS return `NotSearched`, whose `because` says what
MixEngine did — "MixEngine does not search browser certificate databases on this system" — and
**not** that Firefox there reads the system store. That claim may well be true, via
`security.enterprise_roots`, and it is not measured: no machine available to this task has Firefox
on Windows or macOS installed. D14 records how to answer it.

This is also a mechanism decision and not only a scope decision. Windows ships an unrelated
`certutil.exe` in `C:\WINDOWS\system32` — CryptoAPI's, with an entirely different command line. A
PATH-resolved `certutil` on Windows finds the wrong program. Confining the search to Linux means
that collision never arises.

`NotSearched` follows `TrustStoreMethod::None`'s precedent from T49a's D7: a system with nothing to
write is answered, not failed. `.claude/architecture/platform-abstraction.md` rule 4 reserves
`Unsupported` for a capability that was asked for and cannot be given; nobody asked for a Firefox
profile on a machine that has none.

## D3. Where the databases are — six roots, three of which `tls.md` does not have

| Root | Whose |
| --- | --- |
| `~/.pki/nssdb` | Chrome and Chromium, deb or tarball |
| `~/.mozilla/firefox/*/` | Firefox, deb or tarball |
| `~/snap/firefox/common/.mozilla/firefox/*/` | **Firefox snap — Ubuntu 22.04+'s default** |
| `~/snap/chromium/common/chromium/` | Chromium snap |
| `~/.var/app/org.mozilla.firefox/.mozilla/firefox/*/` | Firefox flatpak |
| `~/.var/app/com.google.Chrome/.pki/nssdb` | Chrome flatpak |

A path is a database when it is a directory containing `cert9.db`. Nothing else qualifies — a
Firefox profile directory with no `cert9.db` is a profile that has never been opened, and writing
into one would create a database Firefox may later replace.

**Why six roots rather than the two `tls.md` names.** A glob that matches nothing costs a `readdir`
that returns `ENOENT`. A glob that is missing costs a user a red padlock with no diagnostic, in the
browser they actually use, on the distribution they most likely run. The asymmetry is the whole
argument, and it is why the list is generous rather than minimal.

The home directory comes from `HomeDirs`, not from `$HOME` read directly — `.claude/CLAUDE.md`'s
rule about OS calls, and it is what lets the mock answer.

## D4. Nothing is created

`~/.pki/nssdb` does not exist on a machine where Chrome has never run. mkcert creates one
(`certutil -N --empty-password`). This does not.

A database MixEngine invents is a file in the user's home that no program asked for, holding one
certificate, which the browser may or may not adopt when it first starts and which nothing removes
if MixEngine is uninstalled before then. The browser creates its own on first run; the daemon's
next start finds it and writes; and `mix doctor --repair` is the same write on demand for anybody
who does not want to wait for a restart.

The cost is a real window — install Chrome after MixEngine, open a site, get a red padlock until
something asks again. `mix doctor` names the window in words, which is the difference between a
gap and a silent gap.

## D5. `sql:` only

NSS has two on-disk formats: the legacy Berkeley-DB pair (`cert8.db`) and the SQLite one
(`cert9.db`), selected by a `dbm:` or `sql:` prefix on `certutil -d`. Firefox has written `sql:`
since version 58, in 2018; Chrome's `~/.pki/nssdb` has always been `sql:`.

Every call passes `sql:<dir>` explicitly, and D3's existence test is `cert9.db` and not `cert8.db`.
A legacy database is not found and not written. This is a YAGNI call rather than an oversight: the
legacy branch would be a second code path that nothing here can test and that no supported
distribution produces.

## D6. The nickname carries the key id, and the flags are `C,,`

`certutil -A -d sql:<dir> -n "MixEngine Local CA <key_id>" -t C,, -i <pem>`.

The nickname is the same identity T49a uses in the system stores — T48's key id, the SHA-256 of the
public key, which is what makes two homes on one machine distinguishable and what makes removal
precise. `tls.md`'s bare `-n MixEngine` would have two homes overwriting each other's entry with no
error.

`-t C,,` is "trusted CA for SSL", and only SSL: not email, not code signing. The three positions
are exactly the scope `.claude/architecture/security-model.md` argues for.

**Idempotence is measured, not assumed.** Before writing, `certutil -L -d sql:<dir> -n <nickname>
-a` prints what is already there; its PEM is decoded and compared to the DER byte for byte, the same
test T49a's probes use. Equal means nothing is written. Present and different means the entry is
deleted and re-added — a stale authority under our own nickname is ours to replace, and `certutil
-A` over an existing nickname is not reliably a replacement across NSS versions.

## D7. A missing `certutil` is a state, and it names the package

`NoTool { because }`, where `because` says: `certutil is not installed, so Firefox and Chrome were
not asked — it ships in libnss3-tools`.

Naming the package is the point. "certutil not found" sends a person to a search engine; naming
`libnss3-tools` ends the question. The daemon logs it once at start at `info`, not `warn`: a
machine with no browsers and no NSS tooling is a normal server, and a warning on every start would
be noise on every one of them.

The lookup is `PATH`, resolved once per survey. Not cached across surveys — installing the package
between two daemon starts should work without a restart.

## D8. Every call gets a null stdin and a deadline

This is T49a's macOS lesson applied before it costs anything. `security remove-trusted-cert` waited
twenty minutes in CI printing nothing, because `Command::output()` inherits stdin and has no
timeout; the fix was `Stdio::null()` plus a deadline, and that fix is what found everything after
it.

`certutil` has the same failure available to it: a Firefox profile with a master password set makes
`certutil` prompt for it. Inheriting stdin turns that into a daemon start that never finishes.

So every invocation gets `Stdio::null()` on stdin, piped stdout and stderr drained on threads, and a
per-command deadline — the same `security()` helper shape that
`crates/mixengine-platform/src/macos/trust.rs` already carries, ported rather than reinvented. A
database that would have prompted becomes one line: `this profile asks for a master password`, in
the `because` of that database and nowhere else. The other databases are still written.

**One database's failure never fails the survey or the install.** Each is independent, and a broken
or locked profile is a line in the report, not an error return.

## D9. `cert.ca_status` grows `browsers`, beside `trust` and not inside it

```rust
pub struct CaStatus {
    #[serde(flatten)]
    pub state: CaState,
    pub trust: Trust,
    /// What Firefox and Chrome say — T49b. Separate from `trust` because they are separate
    /// questions with separate answers.
    pub browsers: Browsers,
}

#[serde(tag = "browsers", rename_all = "snake_case")]
pub enum Browsers {
    /// The tool is here; this is what each database found says. May be empty — a machine with
    /// no browser profiles is `Reached { databases: [] }`, not an error.
    Reached { databases: Vec<BrowserDatabase> },
    /// `certutil` is not installed.
    NoTool { because: String },
    /// Not a system MixEngine searches — D2.
    NotSearched { because: String },
    /// The search itself failed.
    Unknown { because: String },
}

pub struct BrowserDatabase {
    /// The directory, so a person can go and look at it.
    pub path: String,
    /// What put it there: `Firefox`, `Firefox (snap)`, `Chrome and Chromium`.
    pub owner: String,
    pub installed: bool,
    /// Why not, or why this one could not be asked.
    pub because: Option<String>,
}
```

Four variants against `Trust`'s four, for the same reason T49a's D10 gives: a client renders what
the daemon returns, and every branch that could not ask says so rather than reporting `false`.

The platform trait's own enum mirrors this without depending on `mixengine-proto` — the existing
`TrustState` / `Trust` split, translated in `crates/mixengine-daemon/src/certs.rs` exactly as
`trust()` translates today.

## D10. The producer is the daemon's start, and it costs no prompt

`main.rs`, immediately after the `require_trust_store` call that T49a added, inside the same match on
a present authority. Same trigger, same bytes, same reason for being at start rather than at first
HTTPS site — M5 requires `https://blog.test` to be green in Firefox and Chrome, and a second command
to make it so is a second command the milestone does not have.

Unlike its neighbour, **nothing is enqueued and no prompt appears**. The write is a subprocess in
the user's own home. It runs on `spawn_blocking` — it is process spawns and file writes — and a
failure warns and does not fail the start, which is the rule every block around it follows.

## D11. A doctor check, and the widening it forces

`ProblemId::BrowsersNotTrusted`, in `doctor.rs` beside `trust_store()`:

- every database holds it, or there are none, or the system is not searched → `Ok`
- `NoTool` → `Note`, naming `libnss3-tools`. Not a problem: a machine that will never have a
  browser would carry a permanent fault, which is the argument the resolver's `hosts_only` check
  already made.
- one or more databases lack it → `Problem`, whose `because` names them
- no usable authority → `Skipped`, as `trust_store()` does

Repair is `Planned::InHome` — no privilege — and that **widens what `InHome` means**. Its doc today
says "everything it touches is under `MIXENGINE_HOME`". NSS databases are under the user's home and
not MixEngine's. The invariant that actually matters is *no privilege*, and the path clause was a
description of the three repairs that existed, not a constraint. The doc gets corrected in this
task rather than the variant being bent to fit it.

Repair asks again; it never regenerates. T49a's D9.

## D12. Removal reuses T49a's reader, and is checked twice

`remove(key_id)` walks every database, reads what sits under our nickname with `certutil -L … -a`,
decodes the PEM, and runs `crate::trust::ours(der)` — the same hand-written DER checks T49a's D4
built. Only a certificate that is MixEngine-shaped **and** whose key id matches is deleted, with
`certutil -D -d sql:<dir> -n <nickname>`.

The nickname alone would be enough to find it and is not enough to delete it. A certificate under a
MixEngine-shaped nickname is not proof MixEngine put it there — T49a's D5, and here the second check
is free because the reader already exists.

**This ships with no producer**, on T42's D12 and T45's D13: T54 (`ca_rotate` / `ca_uninstall`) and
T87 are the producers, and building the removal without one is how it exists and is tested when they
arrive.

## D13. Where the code lives

```
crates/mixengine-platform/src/traits/browsers.rs   BrowserTrust, BrowserTrustState, DatabaseState
crates/mixengine-platform/src/linux/browsers.rs    discovery + certutil, the only real impl
crates/mixengine-platform/src/{windows,macos}/     NotSearched
crates/mixengine-platform/src/mock.rs              answers from memory, records writes
crates/mixengine-proto/src/cert_api.rs             Browsers, BrowserDatabase
crates/mixengine-daemon/src/certs.rs               the translation, beside trust()
crates/mixengine-daemon/src/doctor.rs              the check
crates/mixengine-daemon/src/repair.rs              InHome::TrustBrowsers
crates/mixengine-daemon/src/main.rs                the producer, after require_trust_store
crates/mixengine-cli/src/…                         cert ca-status renders it
```

The `certutil` runner is shared with nothing: `macos/trust.rs`'s `security()` is the shape it is
ported from, not a function it calls, because the two live under different `cfg`s and a shared
helper would have to be compiled on both.

## D14. Testing, and the question this machine cannot answer

**Discovery is unit-tested against a fake home.** `HomeDirs` is injectable, so a temp directory with
`snap/firefox/common/.mozilla/firefox/abc.default/cert9.db` in it is a complete test of D3 — that
the snap root is found, that a profile without `cert9.db` is not, and that six roots resolving to
nothing is an empty list rather than an error. Discovery is a function of the home directory alone,
so no `certutil` is needed to test it; the survey that composes it with D7's tool lookup is the
only part that needs one.

**The round trip is tested against a real `certutil`**, on a database `certutil -N
--empty-password` creates in a temp directory: install, probe finds it, install again writes
nothing, remove takes it out, remove again is a no-op. `.claude/standards/testing.md` rule 1 is
satisfied without a gate, because a temp database is not the user's store — but it is `#[ignore]`d
unconditionally, in the shape the real-Caddy suite already uses, so a developer machine without the
package is not red. CI's `test (ubuntu-latest)` job gains `apt-get install -y libnss3-tools` (about
2 MB) and a step that runs the suite with `--ignored`, which is what makes the round trip a measured
claim rather than an optional one.

**What no machine here can answer: whether Firefox on Windows and macOS needs this too.** The method
is written down rather than guessed at: install Firefox, open `about:config`, read
`security.enterprise_roots`. `true` means it reads the OS store and T49a already covers it; `false`
or absent means those platforms need D3 extended with `%APPDATA%\Mozilla\Firefox\Profiles\*` and
`~/Library/Application Support/Firefox/Profiles/*`, and D2's `NotSearched` becomes a search. The
roadmap entry keeps the question; this task does not invent an answer for it.

## D15. What this task does not do

- **No database is created** — D4.
- **No legacy `dbm:` support** — D5.
- **No producer for the removal** — D12.
- **No Windows or macOS search**, and no claim about Firefox there — D2, D14.
- **No live handshake.** Whether a browser actually accepts a certificate is T53's check; this one
  answers whether the authority is in the database, which is a different and weaker statement, and
  the doctor check's wording says so.
