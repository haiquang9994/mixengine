# T49a — the machine's trust store, and the direction of this operation that can do damage

**Task**: T49a, from [phase 5](../../../.claude/roadmap/phase-5-https.md) — install and remove
MixEngine's CA in the operating system's own trust store, on all three systems, batched into the
single first-run elevation prompt with T42 and T45.

**Authoritative documents**: [`.claude/features/tls.md`](../../../.claude/features/tls.md), the Local
CA section of
[`.claude/architecture/security-model.md`](../../../.claude/architecture/security-model.md), and the
`TrustStore` row of
[`.claude/architecture/platform-abstraction.md`](../../../.claude/architecture/platform-abstraction.md).
Where this design departs from any of them, it says so and says why.

**Depends on**: T48, which put a certificate at `certs/ca/root.crt` and derived its subject's
identifier from the public key. D4 below depends on that derivation being from the key rather than
from the certificate, and would not be possible otherwise.

---

## D1. T49 splits at the privilege boundary, and that is not a preference

The roadmap writes T49 as one task: *"Trust store install/remove per OS, including Linux NSS DBs for
Firefox/Chrome — batched with T42 and T45 into the single first-run elevation prompt."*

Half of that sentence is not true of the other half. `~/.pki/nssdb` and `~/.mozilla/firefox/*/`
belong to the user. Writing to them needs no root, raises no prompt, and cannot be batched into one
— there is no batch to put them in. They go through the daemon directly, not through
`mixengine-elevate`.

So the two halves differ in the one property that decides which binary the code lives in, which
tests can run unprivileged, and whether the work touches the elevation queue at all. That is the
strongest split line available, and it is the same one T15a/T15b and T47a/T47b were drawn on.

- **T49a** — this document. The system store on Windows, macOS and Linux. Privileged, batched.
- **T49b** — the NSS databases on Linux. Unprivileged, in the daemon.

**T49b has a problem of its own that T49a does not, and it is worth recording here so that T49b
starts from a measurement rather than from the specification.** On a stock Ubuntu 24.04:

```
$ which certutil
$ ls -ld ~/.pki/nssdb
ls: cannot access '/home/haiqu/.pki/nssdb': No such file or directory
$ ls -ld /usr/local/share/ca-certificates
drwxr-xr-x 2 root root 4096 Feb 10  2026 /usr/local/share/ca-certificates
```

`update-ca-certificates` is there; `certutil` is not — it ships in `libnss3-tools`, which is not
installed by default. `tls.md` names `certutil` as the mechanism without saying what happens on a
machine that has not got it. T49b has to answer that, and the answer is a reported state rather
than a failure.

**After T49a alone, Chrome and Safari on macOS, and Chrome and Edge on Windows, see a trusted
certificate.** That is what makes it a task that stands on its own rather than half of one.

## D2. Machine-wide, and the asymmetry gets written down instead of discovered

`tls.md` and `platform-abstraction.md` both name the machine-wide store: `LocalMachine\Root` on
Windows, `/Library/Keychains/System.keychain` on macOS. Linux has no choice to make — there is no
per-user system trust store; that is what NSS is for, and it is T49b's.

This is kept, and this document records what it costs, because nothing currently does.

**T48 protects the private key at the scope of one user.** Mode `0600`, a Windows DACL naming the
current user alone, applied by `write_private` as the file is created. **T49a grants the trust at the
scope of the whole machine.** On a machine with more than one account, that means account B's browser
trusts a certificate authority whose private key lives in account A's home directory, and account A
can mint a certificate for any name at all.

`security-model.md` already states the trust model this sits inside — *"a developer tool on a trusted
single-user machine"* — so the case is in scope of a decision already taken. What was missing is that
anybody had said so about this specific asymmetry. `security-model.md` gains a sentence naming it.

The prompt costs nothing extra: T42 and T45 already raise one at first run on every system (on
Windows through NRPT alone, `PortAccessPlan` having no Windows variant, because Windows does not
reserve 80 and 443). Adding the trust install to that batch adds no second prompt, which is the
promise `security-model.md` makes four lines above the sentence this decision amends.

## D3. The DER travels; the path does not

```rust
PrivilegedOp::TrustCaInstall { plan: TrustPlan }
PrivilegedOp::TrustCaRemove  { target: TrustTarget }
```

The wire names `trust-ca-install` and the field `der` are **not invented here**: they are already in
this repository as fixtures, in `crates/mixengine-elevate/src/ops.rs` and
`crates/mixengine-core/src/elevation.rs`, written by an earlier task as the shape a future operation
would take. This design adopts them rather than renaming what two tests already assert about.

**Certificate bytes travel, not a path to a file.** `ResolverPlan`'s doc comment already carries the
argument in full: what the helper can know is compiled into the helper, and only what it cannot know
travels. A path is somebody else choosing which file root reads, after root has decided to trust the
request. The destination store, the file name on Linux, and the update command are all constants in
the helper.

`Vec<u8>`, which serde renders as an array of numbers — about 2 KB of JSON for a 500-byte
certificate. Base64 would be smaller and would need an encoder on both sides; the size is not worth a
dependency, and the array is the shape the existing fixtures already use. **A length cap is applied
anyway**: the field is attacker-controlled in the threat model this binary exists for, and a bound is
one comparison.

One variant per mechanism, mirroring `ResolverPlan` and `PortAccessPlan`:

```rust
pub enum TrustPlan {
    SystemRoot      { der: Vec<u8> },  // Windows: LocalMachine\Root
    SystemKeychain  { der: Vec<u8> },  // macOS:   /Library/Keychains/System.keychain
    CaCertificates  { der: Vec<u8> },  // Linux:   Debian family
    CaTrustAnchors  { der: Vec<u8> },  // Linux:   Red Hat family
}
```

`der` repeats in all four. That is accepted rather than factored into `TrustCaInstall { der, store }`,
because the repetition is one field and the alternative breaks the shape two existing operations
already have — and the reason those have it is that a plan naming a mechanism is a plan the helper
re-validates against the machine it is actually running on.

## D4. What the helper checks, and why a signature check is not among them

`mixengine-elevate` validates every request itself rather than trusting the daemon. For this
operation that means refusing to install a certificate that is not the shape MixEngine's own CA has:

Every one of these is a structural read, which is what makes D11's hand-written reader enough:

| Check | Reason |
| --- | --- |
| parses as X.509 | anything else is not a certificate |
| `issuer == subject` | an authority MixEngine generated is self-signed |
| `basicConstraints = CA:TRUE, pathlen:0` | what T48 generates, and what bounds the chain |
| `keyUsage = keyCertSign, cRLSign` and nothing else | a root that could sign a handshake is not this root |
| no subjectAltName | an authority is not a server — `security-model.md`'s own words |
| `CN == "MixEngine Local CA " + 8 lowercase hex` | see below |
| `notAfter` year − `notBefore` year ≤ 11 | T48's `LIFETIME`, not something a request chooses |
| DER length within a cap | D3 |

**The CN check is checked as a shape, not recomputed from the key, and the first draft of this
design had that wrong.** It said the helper should recompute the key-id — the first 8 hex characters
of the SHA-256 of the SubjectPublicKeyInfo — and require the subject to end with it, calling that
"the check that is worth having". Two things came out of costing it:

1. **It needs SHA-256 inside `mixengine-elevate`.** Measured: `sha2` on its own is 8 crates, none of
   them already in that binary's closure (D11).
2. **It buys nothing.** An attacker generating a certificate sets its CN to the key-id of its own
   key, and the recomputation passes. The binding is trivially satisfiable by exactly the party the
   check would be aimed at.

So the check is what it can actually be: the CN is `MixEngine Local CA ` followed by exactly eight
lowercase hex characters and nothing else. That bounds the subject to an enumerable family, which is
what D4 exists for, and needs no hash.

**T48's key-id is not wasted by this — it earns its keep in D5 instead**, where it is what makes a
removal name one authority precisely.

**The validity check is a comparison of years, not of dates.** Extracting `notBefore` and `notAfter`
as four-digit years and requiring at most eleven between them refuses a hundred-year root, which is
the whole of what this row is for. Full date arithmetic would mean a UTCTime and GeneralizedTime
parser plus a civil calendar in the audited binary, for a refusal it would make no better.

**The self-signature is deliberately not verified**, and this is a decision rather than an omission.
Verifying it needs an X.509 parser and a crypto backend inside the one binary in this workspace that
is excluded from auto-update and is meant to stay small enough to audit by reading it. What it would
buy is nothing: **a daemon compromised badly enough to forge this request already holds the private
key of the CA the machine trusts**, and can therefore sign any certificate for any name without
installing anything. Adding a second CA gains such an attacker no capability it does not have.

So the checks above are not there to stop a compromised daemon. **They are there to bound the set of
things this operation can ever have put into the store**, so that `mix cert ca-uninstall` (T54) and
uninstall (T87) can enumerate that set and be sure they have removed all of it. An unconstrained
install could leave behind a root called anything at all, and nothing would ever find it again.

## D5. Removal is the direction that can do damage, and it is checked twice

The install is close to harmless, for D4's reason. The removal is not.

A `TrustCaRemove` that accepted a fingerprint and deleted whatever matched would let a compromised
daemon **remove the root certificate that validates Windows Update, or a corporate root an
organisation's machines depend on** — through the audited binary, under the user's own Allow click.
That is real damage, and unlike the install it is damage the attacker could not otherwise do.

**The answer is not to defend the fingerprint field. It is not to have one.**

```rust
pub enum TrustTarget {
    SystemRoot     { key_id: String },
    SystemKeychain { key_id: String },
    CaCertificates { key_id: String },
    CaTrustAnchors { key_id: String },
}
```

What travels is T48's **key-id**: eight lowercase hex characters, and the helper refuses anything
else before it looks at a store at all. The removal then finds certificates whose subject is exactly
`MixEngine Local CA <key_id>`, runs the whole of D4's table against each one it found, and removes
only those that pass.

A compromised daemon therefore **cannot name a corporate root**, because no corporate root has that
subject and no eight-hex-character string can be made to describe one. This is `ResolverPlan`'s own
argument applied one capability along: the value an attacker would abuse is not validated, it is
absent.

**This is where T48's key-id earns its keep**, having turned out (D4) to be worth nothing as an
install-time check. It is derived from the public key, so two homes on one machine have different
ones, and a removal is precise to a single authority rather than to every MixEngine-shaped
certificate in a machine-wide store.

**What this costs, said plainly: T54 has to sequence a rotation.** Removing the old authority and
installing the new one are two operations rather than one, and the queue's `dedupe_key` (D3) makes
them supersede each other inside a single batch. T54 will need either two grants or a dedupe key that
distinguishes them. That is a real constraint this decision imposes on a later task, and it is
smaller than a field a compromised daemon could use to disarm the machine's own updater.

On Linux the file name is a constant compiled into the helper, so the target still names no path; the
helper reads the file and checks its contents against D4 before unlinking it, because a file at that
path is not proof that MixEngine wrote it.

**Nothing in T49a enqueues a removal.** This follows T42's D12 and T45's D13 exactly: the operation
ships built, validated and tested, with no producer. T54 (`cert.ca_rotate`, `ca_uninstall`) and T87
(uninstall) are the producers. The reason is the one both earlier tasks give — reversing a mechanism
five phases after it was written is a worse task than writing both halves while the mechanism is in
view.

## D6. Three mechanisms, and reading each one costs no privilege

| System | Write | Read |
| --- | --- | --- |
| Windows | `CertAddEncodedCertificateToStore` into `LocalMachine\Root`, through `windows-sys` | enumerate the store, compare DER bytes exactly |
| macOS | `/usr/bin/security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain <file>` | `security find-certificate -a -Z` |
| Linux | write `mixengine.crt` into the anchors directory, then `update-ca-certificates` or `update-ca-trust` | the file holds our bytes **and** the generated bundle contains them |

**Windows goes through the API rather than `certutil.exe`.** `tls.md` names CryptoAPI first with
`certutil` as a fallback; the fallback is not built. `mixengine-platform` already reaches Windows
through `windows-sys` for the resolver's registry work and for the named pipe's DACL, and a process
spawned from an elevated context is a larger surface than four API calls.

**macOS shells out to `security`, and the elevate binary stays as it is.** The alternative —
`SecCertificateCreateWithData`, `SecItemAdd` and `SecTrustSettingsSetTrustSettings` through FFI — is
a new unsafe surface in the binary whose whole design constraint is that a person can audit it by
reading it, for an operation that runs once per install. The rule T42 set with `pfctl` and T45 kept
with `systemctl` holds: **one fixed command, a constant argument vector, and no argument taken from
the request**. The file path passed to `security` is written by the helper into a location the helper
chose, from the DER the helper has already validated — it is not a path the request supplied.

**Linux has no API at all**, so the subprocess there is forced rather than chosen.

**The reads matter as much as the writes.** The producer runs on every daemon start and the doctor
check runs whenever somebody asks, so both depend on reading the store being cheap and needing no
privilege — exactly the property `require_port_access` and `require_resolver` depend on. D13 records
that two of the three reads are assumptions this machine cannot test, and how they get measured.

**Comparison is by exact DER bytes, not by subject or by a hash property.** A subject match would
claim another home's authority as this one's; a store's own SHA-1 property is a different value from
the SHA-256 `cert.ca_status` reports, and carrying two hashes for one identity is how they come
apart.

## D7. On Linux the mechanism is a runtime question, and "none" is an answer

macOS is always the system keychain and Windows is always `LocalMachine\Root`. Linux is one of two
families or neither, so `TrustStore::method` returns a `Result` for the reason
`ResolverConfig::method` does (the T45 design, D2) while `PortAccessMethod` is a constant.

```rust
pub enum TrustStoreMethod {
    SystemRoot,
    SystemKeychain,
    CaCertificates,
    CaTrustAnchors,
    None,
}
```

**Detected by probing for the directories, never by parsing `/etc/os-release`** — `tls.md` says so
in a sentence of its own, and a version string is a thing distributions change.

`None` is a valid answer and not an error, exactly as `ResolverMethod::None` is: a machine with
neither directory keeps working over HTTP, `cert.ca_status` says why in words, and nothing fails.

## D8. The producer, and where it sits

```rust
Elevation::require_trust_store(&self, der: Option<&[u8]>) -> Result<(), Error>
```

The DER arrives as an argument rather than by `Elevation` reaching for `Certificates`, mirroring
`require_port_access(binary: Option<&Path>)`. `None` — a home whose authority could not be made —
asks for nothing.

Called in `serve()` from the block that already asks for ports and the resolver, **after** T48's
`Certificates::ensure`, because it needs a certificate to exist. Enqueue order inside one batch does
not matter; existence does.

A probe that fails asks for nothing and warns, which is `require_resolver`'s rule and not
`require_hosts`': the helper is not the authority on what the store holds, so a read that failed has
said nothing about what to ask for.

Whole state, like every operation beside it: `TrustState::plan` returns `None` when the store already
holds exactly this certificate, so a second start does not put a row on `mix status` whose only
possible outcome is `AlreadyDone`.

## D9. A doctor check, and why this is the opposite of T48's answer

`ProblemId::CaNotTrusted`, repaired by `Planned::Enqueue(Enqueue::TrustStore)`.

**T48 refused to add a check, and was right to.** There, the condition was a damaged authority and
repairing it would have meant regenerating — destructive, invalidating every leaf and every store
holding the old certificate, and therefore T54's decision to make rather than T48's.

Here the condition is "the machine does not trust it yet" and the repair is *ask again* — which is
precisely what `ResolverNotWired` and `PortAccessMissing` already do, through the same `Enqueue`
arm. The two answers differ because the conditions differ, not because the tasks disagreed.

A machine whose `TrustStoreMethod` is `None` produces `Outcome::Skipped` with the reason in words,
never a problem: there is nothing to repair on a machine that has no store.

## D10. `cert.ca_status` grows the field T48 left out on purpose

T48's own record says it: *"no trust-store field on `cert.ca_status` — that is about the operating
system and is T49's, and a field this build could only fill with 'unknown' is not an answer."*

It is now answerable, so it is answered. The field sits beside `Ca` rather than inside it — whether
this machine trusts a certificate is a fact about the machine, and `Ca` is a description of the
certificate.

```rust
pub struct CaStatus {
    #[serde(flatten)]
    pub state: CaState,
    pub trust: Trust,
}

#[serde(tag = "trust", rename_all = "snake_case")]
pub enum Trust {
    Installed { store: String },
    NotInstalled { store: String },
    NoStore { because: String },
    Unknown { because: String },
}
```

`Unknown` exists because a read that failed is a real outcome and the only honest thing to print
then. `NoStore` is D7's Linux machine. Both carry their reason in words, for the screen.

`mix cert ca-status` renders one line for it. The exit code stays zero in every state, for T48's
reason: this command reports, and `mix doctor` is what carries a verdict.

## D11. The obvious dependency was measured and refused, so D4's checks are hand-written

The first draft of this design reached for `x509-parser`, which is already a workspace dependency
after T48. **That was wrong, and measuring it is what showed why.**

`mixengine-elevate`'s dependency closure is pinned in
[`.github/elevate-dependencies.txt`](../../../.github/elevate-dependencies.txt), which CI regenerates
and diffs, and whose first three lines say *"Everything here runs as root. Adding a line is a
security decision."* Measured, on this machine, both built on their own:

| | crates |
| --- | --- |
| `mixengine-elevate` today | 18 |
| `x509-parser` alone | 29 |
| in common | 7 |

So the honest figure is **18 → about 40**: adding one line to that file more than doubles what runs
as root, and brings in `nom`, `der-parser`, `asn1-rs` with two proc-macro crates of its own,
`num-bigint`, `time` with three, `oid-registry` and `data-encoding` — a general-purpose X.509 parser
built to *understand* certificates, in a binary whose job is to *refuse* them.

**So the checks are written here, against a minimal DER reader of our own**, in
`mixengine-platform/src/trust/der.rs`.

This is affordable because of what D4 actually asks for. The helper never needs to understand a
certificate: it needs to walk to seven places and refuse anything else. A tag-length-value reader
over a byte slice is the whole mechanism — read a tag, read a length, hand back the contents as a
subslice, and refuse every encoding it was not written for. Indefinite lengths, lengths that do not
fit, tags it does not know: all refused rather than skipped. `issuer == subject` is a comparison of
two subslices and needs no name parsing at all.

It cannot be a memory-safety problem — it is safe Rust over `&[u8]`. It **can** be a panic, which in
this binary means no response file, so every read goes through `get()` and never through indexing,
and a test feeds it truncated and malformed input to prove it answers rather than unwinds.

The residual risk is a reader that accepts something it should not, and D4's last paragraph already
bounds what that costs: the checks exist to keep the set of installable certificates enumerable by
uninstall, not to stop an attacker who already holds the CA key. A reader bug means uninstall could
miss something. That is a real cost and a bounded one, and it is smaller than 22 more crates running
as root.

`x509-parser` stays where T48 put it — a **dev-dependency**, used by the tests to read back what the
hand-written reader accepted, which is the arrangement that makes the two check each other.

**`sha2` is not added either, and costing it is what corrected D4 and D5.** The helper was going to
need SHA-256 to recompute a key-id on install; measured, that is 8 more crates with **no** overlap
with the 18 already there. Asking what those 8 crates bought produced the answer that the install
check they were for is trivially satisfiable by the attacker it was aimed at (D4), and that the
removal — the direction that can actually do damage — is safer naming an authority by its key-id than
by a hash the helper would have to compute (D5). **The closure stays at 18 crates and
`.github/elevate-dependencies.txt` gains no line**, which is the outcome that file exists to make
somebody argue for.

`windows-sys` gains `Win32_Security_Cryptography`, for the four store calls in D6. It is already a
non-optional dependency of `mixengine-platform` on Windows, so this adds a feature and no crate.

**No crypto backend is added either**, which is the whole point of D4's last paragraph.

## D12. Where the code lives

```
mixengine-proto/src/privileged.rs      TrustPlan, TrustTarget, the two ops
mixengine-platform/src/trust/mod.rs    the trait, TrustStoreMethod, TrustState
mixengine-platform/src/trust/der.rs    the tag-length-value reader of D11 — pure, no dependency
mixengine-platform/src/trust/check.rs  D4's table — pure, compiled everywhere, tested everywhere
mixengine-platform/src/{windows,macos,linux}/trust.rs   the reads and the writes
mixengine-elevate/src/trust.rs         apply and remove, and nothing else in the file
mixengine-daemon/src/elevation.rs      require_trust_store
mixengine-daemon/src/{doctor,repair}.rs   CaNotTrusted and its arm
mixengine-proto/src/cert_api.rs        Trust
mixengine-cli/src/render.rs            one line
```

`check.rs` is pure and compiled on all three systems, following `resolver/` and `port_access/`: a
developer on any one machine can test the validation for all three, and the alternative — putting it
inside `windows/` — is a check that only a third of this project's machines can run a test against.

## D13. Testing, and the two things this machine cannot answer

D4's table, the plan and target construction, and each system's generated values are unit-tested and
run everywhere.

**The validation is tested by the certificate it must refuse, not only by the one it must accept.**
One case per row of D4's table, each a certificate that is correct in every way but one — a real CA
with a SAN, one with `digitalSignature` added, one whose CN carries a key-id from a different key.
The accepting test alone would pass against a function that returns `Ok` unconditionally. Each of
those certificates is **generated with `rcgen`**, the same way T48 generates the real one, so the
refusals are refusals of real certificates rather than of hand-edited bytes.

**The hand-written reader of D11 is tested against `x509-parser`, which is what makes the pair worth
having.** For every certificate the reader accepts, the dev-dependency parses the same bytes and the
test asserts they agree about the subject, the issuer and each extension. A reader that quietly
disagreed with a real parser is the failure mode this arrangement exists to catch, and neither half
alone can.

**And the reader is fed input designed to make it panic**, because a panic in this binary means no
response file at all: truncated at every byte offset of a valid certificate, indefinite lengths,
lengths that overflow, tags it does not know, and a zero-length slice. The assertion is that it
returns an error — any error — for every one of them.

**Anything that touches a real store is a system test**: `#[ignore]`d and gated on
`MIXENGINE_SYSTEM_TESTS=1`, run by CI's existing `system` job on all three systems. That is the
exception `.claude/standards/testing.md` rule 1 names, and it names the trust store by name.

**Two assumptions this design rests on cannot be measured on the machine it was written on, and are
not assumed in a comment.** Both become real tests, in the *unprivileged* job:

1. **Enumerating `LocalMachine\Root` without an administrative token.** `test (windows-latest)` runs
   as an ordinary user; if the read needs elevation, the producer and the doctor check both need a
   different shape, and this is where that is found out.
2. **`security find-certificate -a -Z /Library/Keychains/System.keychain` as an ordinary user.** Same
   reasoning, in `test (macos-latest)`.

A test that finds no MixEngine certificate is the expected result on a clean runner; what is being
proved is that the read **succeeds**, not what it returns.

**The Linux read is measurable here** and was: `/usr/local/share/ca-certificates` is
`drwxr-xr-x root root`, and `/etc/ssl/certs/ca-certificates.crt` is world-readable.

## D14. What this task does not do

- **The NSS databases.** T49b, split at D1's line.
- **Enqueue a removal.** D5 — built, validated, tested, no producer. T54 and T87.
- **Issue any leaf certificate.** T50. Nothing is signed with this CA yet.
- **Serve HTTPS.** T51 wires the web server, and explicitly disables Caddy's automatic ACME.
- **`mix cert status`.** Still left free for T53's per-site handshake, with T48's test still asserting
  that it fails.
- **Firefox on Windows and macOS.** `tls.md` names NSS on Linux only, and whether Firefox on the
  other two reads the system store depends on `security.enterprise_roots`. **This design does not
  claim an answer**, because it has not measured one. It is written down here as a question T49b or a
  later task must answer by measurement, not as a gap nobody noticed.
