# T48 — the internal CA, and the identifier the specification asked for and cannot have

**Task**: T48, opening [phase 5](../../../.claude/roadmap/phase-5-https.md) — internal CA generation
(`rcgen`), key permissions, fingerprint, `cert.ca_status`.

**Authoritative documents**: [`.claude/features/tls.md`](../../../.claude/features/tls.md) and the
Local CA section of
[`.claude/architecture/security-model.md`](../../../.claude/architecture/security-model.md). Where
this design departs from either, it says so and says why.

---

## D1. The certificate cannot carry its own fingerprint, so the short identifier comes from the key

`security-model.md` asks for the subject `MixEngine Local CA <short-fingerprint>`. A certificate's
fingerprint is a hash *of the certificate*, and the subject is inside the certificate — so the value
would have to be known before the bytes that determine it exist. There is no ordering that produces
it.

**The short identifier is the first 8 lowercase hex characters of the SHA-256 of the
SubjectPublicKeyInfo**, which is computable the moment the key pair exists and therefore before
anything is signed. It is also the more useful of the two to have in a name: it survives re-signing
the same key, which is what makes two certificates for one CA recognisable as one CA.

**`ca_status` reports a different number, deliberately: the SHA-256 of the certificate DER.** That is
what a browser shows, what a trust store lists, and therefore the only number a person can actually
compare against something. Both appear, and the documentation says which is which — an identifier
that looks like a fingerprint and is not one is worse than either alone.

`security-model.md` is amended to match rather than left to be discovered.

## D2. What is on disk, and in which order

```
certs/ca/root.key   PKCS#8 PEM, private
certs/ca/root.crt   PEM, public
```

**The key is written first.** A crash between the two writes then leaves a key with no certificate,
which is a state that can be recognised and refused; the other order leaves a certificate with no
key, which is the same shape as a certificate whose key was lost and is one T50 could try to issue
against.

Neither file is written through a temporary-file rename. That protection exists in
`mixengine-elevate` for writes into files other programs own, where a half-written `/etc/hosts` is a
broken machine. Here both files are MixEngine's own, in MixEngine's own directory, and a torn write
is one of the unusable states D5 already has to report. Adding a second mechanism to avoid a state
that must be handled anyway buys nothing.

## D3. `write_private` creates the file already restricted, and it is a new platform primitive

Nothing in this workspace can write a file with restricted permissions today. `DirectoryAccess` has
exactly two methods and both are about directories, and `restrict_to_owner` is applied only to the
four private directories at bootstrap.

**`mixengine_platform::write_private(path, bytes)`**, a free function beside `generate_secret`, which
is where the crate already puts an OS primitive that belongs to no `Host` capability.

- **Unix**: `OpenOptions::new().mode(0o600).create_new(true)`. The mode is applied *at creation*, so
  there is no instant in which the private key exists at the umask's mode. `fs::write` followed by
  `set_permissions` would leave exactly that instant.
- **Windows**: write, then sever inheritance and grant the current user, `SYSTEM` and
  `Administrators` — the same three `restrict_to_owner` leaves. **It cannot reuse that method.**
  `windows/access.rs` grants `(OI)(CI)F`, and Object Inherit and Container Inherit are directory-only
  flags that `icacls` refuses on a file.

T3b's note on this task says "keep the order, or restrict the key file itself". Both are done. The
order already holds — `Paths::bootstrap` restricts `certs/` before anything is written into it — but
relying on it alone makes the key's protection a property of a directory whose loss `mix doctor`
already has a name for (`HomePermissionsLost`). One of those is a fact about this file; the other is
an invariant somewhere else.

## D4. Generated at daemon start, and the two documents disagree about this

`security-model.md` says "generated on first use". `tls.md` puts generation at step 1 of first-run
setup, with the trust-store install batched into **the single** first-run elevation prompt. Both
cannot hold: if the CA appears when the first HTTPS site is created, T49's trust-store install is a
second prompt, and `security-model.md`'s own "expected lifetime total: one prompt at first run" is
already broken by its own sentence four lines earlier.

**T45 settled this exact question one subsystem over and wrote down why** — the resolver wiring is
asked for at daemon start, before any site exists, because asking afterwards turns one operation into
two and therefore one prompt into two. "On first use" is wording that predates that finding.

So: **ensured at daemon start**, immediately after `Paths::bootstrap`, in the same block as T42's port
probe and T45's resolver probe and under the same rule they state — a failure is a `tracing::warn!`
and never a refusal to start, because a machine that was not set up is one command away from being
set up, where a daemon that will not start leaves the user with nothing.

The cost is an ECDSA P-256 key on disk in homes that never serve HTTPS. That is one file and about a
millisecond, against a second UAC prompt for everyone who does.

`security-model.md`'s wording is amended.

## D5. A broken CA is reported and never silently replaced

Regenerating on finding damage would invalidate every leaf certificate already issued and every trust
store the old CA was installed into, in response to no request from anybody. Rotation is **T54**, and
it exists precisely because it has the steps this would skip: reissue the leaves, remove the old
certificate from the stores.

So `ensure` at start creates a CA only when there is none at all. Everything else is reported.

`Absent` therefore survives D4: a start whose `ensure` failed warns and carries on, and the next
`cert.ca_status` says the home has no CA rather than inventing a reason it could not read.

**The reasons are a closed enum, not a string** — T47a's `ProblemId` rule, for its reason: a client
that matches on wording is a client that silently stops matching.

```rust
pub enum CaState {
    Absent,
    Present(Ca),
    Unusable(Unusable),
}

pub enum Unusable {
    KeyMissing,
    CertificateMissing,
    KeyUnreadable,
    CertificateUnreadable,
    KeyAndCertificateDisagree,
}
```

`KeyAndCertificateDisagree` earns its place rather than being defensive: the SubjectPublicKeyInfo in
the certificate is compared against the public key derived from `root.key`, and a home restored from
a backup that caught one file and not the other is exactly how they come apart. Without the check
the symptom appears three tasks later, as leaf certificates no browser trusts.

An expired CA is **not** unusable. It is `Present` with a `days_left` that has gone negative, because
that is a true statement about a certificate that exists and parses, and because the repair for it is
rotation rather than anything this build offers.

## D6. `cert.ca_status`, and the field it does not have

```rust
pub struct CaStatus { pub state: CaState }

pub struct Ca {
    pub subject: String,
    pub fingerprint: String,        // SHA-256 of the DER, lowercase hex, no separators
    pub key_id: String,             // the 8 hex characters D1 puts in the subject
    pub not_before: Timestamp,
    pub not_after: Timestamp,
    pub days_left: i64,
    pub certificate_pem: String,
}
```

`certificate_pem` is the public certificate and nothing else. `security-model.md` says the private key
is never copied, exported by an RPC, or sent to a client, and there is no field here it could travel
in.

**There is no trust-store field.** Whether the OS trusts this certificate is a question about the
operating system rather than about the CA, it is answered by machinery **T49** builds, and shipping a
field that this build can only answer "unknown" would be shipping an answer nobody can use. This is
`DnsStatus`'s shape from T46: report the independent facts, refuse to collapse them into a verdict.

`days_left` is derived rather than stored, and is `i64` because it goes negative.

## D7. `mix cert ca-status`, not `mix cert status`

`tls.md` gives `mix cert status` to the per-site diagnostics with a live handshake, which is **T53**.
The CA is a different noun, and `tls.md` already names its siblings `mix cert ca-uninstall` and
`mix cert ca-rotate`. Taking the shorter name now would mean renaming it in T53 or giving one command
two unrelated jobs.

## D8. Dependencies

| Crate | Version | New package? |
| --- | --- | --- |
| `rcgen` | 0.14.9, default features plus `x509-parser` | **No.** Its default backend is `ring`, and `ring` and `aws-lc-rs` are both already in `Cargo.lock` |
| `x509-parser` | 0.18 | **Yes**, and already named in `rust.md`'s table. `rcgen` pins `0.18`, so one copy and not two |

`sha2` is the workspace's existing 0.10, per `rust.md`'s pin — not the 0.11 that is also in the tree
underneath something else.

The `x509-parser` feature on `rcgen` is enabled here although only **T50** calls what it unlocks
(`Issuer::from_ca_cert_pem`, which is how a stored CA signs a leaf). It costs nothing now — the
package is required by D6 regardless, for reading the validity window off the file — and it is what
makes D2's storage shape one T50 can load.

## D9. Where the code lives

| File | Holds |
| --- | --- |
| `crates/mixengine-platform/src/{unix,windows}/private_file.rs` | D3, per OS |
| `crates/mixengine-core/src/certs/ca.rs` | generation, loading, the two fingerprints, the agreement check. No OS calls |
| `crates/mixengine-proto/src/cert_api.rs` | D5 and D6's types, `CERT_CA_STATUS` in `rpc.rs` |
| `crates/mixengine-daemon/src/certs.rs` | the start-up `ensure`, the RPC handler |
| `crates/mixengine-cli/src/main.rs`, `render.rs` | `mix cert ca-status`, both renderings |

## D10. Testing

**Generation is judged by parsing the result, never by restating the input.** The certificate is read
back with `x509-parser` and asserted on: `basicConstraints` CA with `pathlen:0`, `keyUsage` exactly
`keyCertSign|cRLSign`, a P-256 public key, a validity window that begins at generation and is ten years wide, and a subject whose
common name ends in the eight characters D1 derives from the key. Asserting on the `CertificateParams`
that were passed in would prove that `rcgen` was called and nothing about what it produced.

**The fingerprint is compared against an independently computed hash** of the DER, not against
whatever the code that produces it produces.

**Idempotence is byte-identity.** A second `ensure` over a complete `certs/ca/` leaves both files with
the same bytes — not merely "succeeds", which a silent regeneration would also do.

**Each unusable state is built, not imagined.** Delete `root.key`; truncate `root.crt`; write a
certificate generated from a *different* key pair beside the first key. Each must be reported as
itself, and `KeyAndCertificateDisagree` in particular must not arrive as `CertificateUnreadable`.

**`write_private` runs under `umask(0o000)` on Unix.** Under a permissive umask a plain `fs::write`
yields `0666`, so asserting `0600` proves the mode came from this code. Without setting the umask the
same assertion passes on a developer machine whose umask happens to be `0o077`, and would keep
passing if somebody replaced the implementation with `fs::write` — a green test measuring the
machine. On Windows the assertion is that the file's own ACEs carry no inherited flag and number
three, which is what `windows/access.rs` already knows how to read.

**The daemon's start-up path is proved end to end**, on a real socket: start a daemon over an empty
home, call `cert.ca_status`, and get `Present` with a fingerprint — the ordering claim of D4 is
worthless if only a unit test ever calls `ensure`.

## D11. What this task does not do

- **No trust-store anything.** T49.
- **No leaf issuance.** T50, which is what D8's feature choice and D2's storage shape are for.
- **No rotation and no regeneration**, argued in D5. T54.
- **No `mix doctor` check** for a missing or expired CA. The doctor asks nothing about certificates
  today, and adding a `ProblemId` means deciding what repairing it means — which is T54's decision,
  not a corner of this one.
