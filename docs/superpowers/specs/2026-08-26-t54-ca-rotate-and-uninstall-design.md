# T54 — `cert.ca_rotate` and `cert.ca_uninstall`

**Roadmap task:** T54, the last of phase 5.
**Status:** design, agreed 2026-08-26.

## What this is for

Phase 5 has spent five tasks putting a certificate authority into this machine and keeping the
leaves under it fresh. T54 is the only task that takes something back out.

Two operations, and they are the same shape:

- **`cert.ca_rotate`** — replace this home's authority with a new one. What it is for is a key that
  may have leaked and an authority that has run out of its ten years. It is destructive: every
  browser holding a cached chain under the old authority stops accepting it.
- **`cert.ca_uninstall`** — take this home's authority back out of every store that trusts it,
  leaving the files alone.

Both are already half-built. `PrivilegedOp::TrustCaRemove` has been implemented and tested in
`mixengine-elevate` since T49a, and `BrowserTrust::remove` since T49b. **Neither has ever had a
caller**, and both say so in their own documentation: they were built with T54 named as the
producer, so that this task would have a mechanism to build against rather than one to invent
against code written phases earlier. T54 is mostly wiring and ordering.

## D1 — Both methods are jobs, and the reason is the prompt

`cert.ca_rotate` and `cert.ca_uninstall` answer a `JobSummary`, as `elevation.grant` does, rather
than a report.

Both have to wait for an elevation prompt, and a prompt is not a thing an RPC can block on: the
person may take a minute, or walk away. `mix` already follows a job to completion and streams its
progress (`follow`, used by `mix elevation grant` and `mix doctor --repair`), so from the keyboard
this is still one command, one prompt, one result. The asynchrony is the daemon's, not the person's.

## D2 — The commit is decided by measuring the store, never by the prompt's own report

After the grant ends, the job **re-probes** the trust store with `TrustStore::probe` and re-surveys
the browser databases with `BrowserTrust::survey`. What those return is what the report carries, and
for a rotation it is also what decides whether anything is committed at all.

`mixengine-elevate` reports what it did, and it is honest, but it is a separate process reporting on
work it has finished. A probe is a fresh reading of the thing itself, it needs no privilege on any
of the three systems — which `mixengine-platform/tests/trust.rs` measures rather than asserts — and
it costs nothing worth counting once per rotation. `tls.md`'s acceptance criterion asks for exactly
this: *"leaves no MixEngine certificate in any store (verified by an integration test that
enumerates the stores)"*.

This is the same rule T49b arrived at the hard way. Three list-reading measurements pointed the
wrong way there before a live handshake corrected them, and T53 exists because reading a file is not
the same claim as asking the thing that holds it.

## D3 — A rotation stages the new authority, and promotes it only after D2 agrees

```
certs/ca/root.key          the authority this home has now
certs/ca/root.crt
certs/pending/ca/root.key  the candidate
certs/pending/ca/root.crt
```

**The staging area is a certificates directory of its own, not a subdirectory of `certs/ca/`.** That
one choice is what makes the rest of this free: every function in `ca.rs` takes the certificates
root as its first argument, so `ca::ensure(&certs.join("pending"), now)` *generates the candidate*
and `ca::read(&certs.join("pending"), now)` *describes it* — with no second code path for making an
authority, and therefore no way for the candidate to be made differently from the real one.
Promoting is two file moves; discarding is one `remove_dir_all`.

Nothing else can see it. `ca::read` reads two exact paths and does not glob, and leaves live under
`certs/sites/`, so `certs/pending/` collides with nothing: `cert.ca_status`, `cert.issue`, the
renewal loop and `mix doctor` all go on seeing the authority this home has now.

The sequence:

1. Discard any stale `certs/pending/` (D10).
2. Generate a new key pair and self-signed certificate into it.
3. Read the old authority's `key_id` and DER, before anything is overwritten, and probe the store
   with that DER — **this reading is D7's third clause**, and it cannot be taken afterwards, because
   by then the removal has already run.
4. Enqueue `TrustCaRemove { old key_id }` and `TrustCaInstall { new der }` — one grant, D4.
5. Run the grant inside this job (D5).
6. Probe (D2). If the answer is no, delete `certs/pending/` and stop: **this home is exactly as
   it was**, and the report says why.
7. Otherwise commit: move the pending pair over `certs/ca/root.*`, reissue every leaf (D6), put the
   new authority into the browser databases and take the old one out, then regenerate configuration
   so the front end re-reads.

The window in which a site is broken is between step 7's first and second lines, and it is inside
one job. Nothing outside the job can observe a home that has a new authority and old leaves.

## D4 — One grant covers remove-and-add together

The queue gets both operations before the prompt is raised, so a rotation costs one elevation and
not two.

This is not a convenience, it is the argument phase 5 already made and recorded. Moving the trust
store per-user was measured on Windows and macOS on 2026-08-25 and rejected: Windows raises
CryptoAPI's own Security Warning for a write *and* for a removal, and macOS asks for the account
password twice, once to add and once to remove. The roadmap entry for T49a states the cost in T54's
terms — *"one `ca_rotate` would cost two password prompts where a single elevation grant covers
remove-and-add together"*. Splitting the grant here would spend the thing that argument bought.

## D5 — `Elevation::grant_within`, the one change to the elevation machinery

`Elevation::grant` today does its checks, takes the single grant slot, and then begins a **job of its
own** whose body is `flush`. A rotation cannot use it: the commit has to happen after the flush and
inside the same job, and there is no hook between one job ending and another beginning.

`grant` splits into the part that decides and the part that runs:

```
preflight()        -> (Vec<PendingOp>, PathBuf)   the queue, the helper, whether this machine can
                                                  prompt at all, and the grant slot, reserved
grant()            = preflight + begin its own job + flush        unchanged from outside
grant_within(&h)   = preflight + flush inside the caller's job    new
```

`flush` already releases the slot however it ends, through a `Drop` guard rather than a last
statement, so a rotation that panics between the grant and the commit does not wedge every later
grant for the life of the daemon. That guard is why this split is safe to make.

Without `grant_within` the commit would have to live outside the job, and nothing would guarantee it
ran after permission was given — which is precisely the property this whole design exists to have.

## D6 — Reissuing the leaves is `issue(None)` and no new code

T50 gave certificate reuse a fourth question: *was this leaf signed by the authority this home has
now?* The comparison is the leaf's issuer name against the authority's subject name, free because
T48 put the key's identity into that name.

That question was added for T54 and `tls.md` says so — *"Without it, T54's rotation leaves every site
holding a leaf that parses, covers the right names and has eighty days left, and that no browser
accepts."* The consequence is that the moment `certs/ca/root.crt` is a different authority, every
leaf on disk is stale by the existing rule, and `Certificates::issue(None)` reissues all of them.

T54 writes no reissue logic. If it needed to, that would be evidence T50's fourth question was
wrong.

## D7 — When a rotation commits, in four clauses

Step 6 cannot simply ask "is the new authority in the store", because a machine with no store
MixEngine knows how to write would then never be able to rotate — and that machine is supported, not
broken. The condition is **whether this machine is less able to trust the new authority than it was
to trust the old one**:

| After the grant | Commit? | Why |
| --- | --- | --- |
| The store holds the new DER | yes | the rotation did what it set out to do |
| `TrustStoreMethod::None` | yes | there is no store to be worse than; Linux reaches its browsers through NSS |
| The store did not hold the old DER either, **read in step 3** | yes | this store was never trusting ours; the rotation changes nothing about it |
| The probe could not be read | **no** | a failed read has said nothing, and doubt is free here |
| Otherwise | **no** | the old authority was trusted, the new one is not, and committing would break every site |

The fourth row differs from the rule the rest of this repository follows — `require_trust_store` and
`require_port_access` both treat a failed probe as "ask for nothing and carry on". They can, because
what they do next is harmless. This is the one destructive operation in phase 5, and the staging
design (D3) makes refusing cost nothing but a deleted directory.

## D8 — `cert.ca_uninstall` takes trust and never a file

It removes this home's authority from the system trust store and from every NSS database that holds
it. `certs/ca/root.crt`, `certs/ca/root.key` and every leaf under `certs/` are left exactly as they
are.

Removing trust is reversible — `mix doctor --repair` puts it back, because T49a's `CaNotTrusted`
condition already exists and its repair is *ask again*. Deleting a private key is not reversible by
anything. Deleting the files is what T87 does, when the whole product is being removed and the
question of a home that still wants HTTPS does not arise.

**And uninstall is not all-or-nothing where rotation is.** Each store is independent, and taking the
authority out of Firefox is a complete action on Firefox whatever the system store did. So a
declined elevation prompt still leaves the browser databases cleaned, and the report says the system
store still holds it. A rotation cannot take that shape: a home with a new authority and half the
machine trusting the old one serves leaves nobody accepts.

The browser databases need no privilege in either direction — that is the line T49 was split on —
so they are handled in the job, outside the grant, in both methods.

## D9 — A damaged authority can be rotated; an absent one cannot

`ca.rs` refuses to repair damage and points at this task for the reason: *"Rotation is roadmap task
T54, and it exists because it has the steps this would skip: reissue the leaves, then remove the old
certificate from the stores."* So rotation is the documented repair for a `CaState::Damaged`, and it
must work there.

- **Damaged, `key_id` readable** — rotate, and enqueue the removal.
- **Damaged, `key_id` unreadable** — rotate without a removal, and say in the report that the old
  certificate was left in the store because nothing could name it. Never guess a target: T49a's D5
  is that a removal names an authority by key-id and never a certificate by fingerprint, because a
  removal that could name a fingerprint could name the root that validates Windows Update.
- **Absent** — refuse, `PreconditionFailed`, hint `mix doctor --repair`. There is nothing to rotate,
  and making one is what a daemon start already does.

## D10 — Stale staging is discarded, twice

`certs/pending/` holds a private key that nothing uses. A crash between generating it and
committing would leave it there for as long as the home exists.

It is deleted at the start of every rotation, and at daemon start alongside the existing
`Certificates::ensure` call. The daemon is single, so at start there is no rotation in flight and
the delete cannot race one.

## D11 — The queue is shared, and that is the caller's to read

`mix cert ca-rotate` flushes **everything** waiting, not only the two operations it added. If a
hosts-file change was queued and never granted, this grant applies it too.

That is right rather than merely unavoidable. T64's rule is that what is about to be allowed is read
before it is allowed, and `mix` already has the machinery: `confirmed(&waiting, json)` prints every
pending operation and what each will literally change, then asks. Both commands use it, and both
take `--yes` to answer in advance, exactly as `mix elevation grant` and `mix doctor --repair` do.

It does mean `mix cert ca-rotate` can change the hosts file, and that whoever runs it must read the
list rather than the command name. The alternative — a private queue per operation — is a second
queue to keep in step with the first, and ADR 0005's rule that no code path elevates in a loop is
enforced by there being one slot over one queue.

## D12 — What travels on the wire

```rust
/// cert.ca_rotate — takes nothing.
pub struct CaRotateQuery {}

/// cert.ca_uninstall — takes nothing.
pub struct CaUninstallQuery {}
```

Both answer a `JobSummary`. The job's result value is one of:

```rust
pub struct CaRotateReport {
    /// What happened.
    pub outcome: RotateOutcome,

    /// The authority that was replaced.
    ///
    /// `None` for a home whose authority was damaged: `CaState::Damaged` carries no `Ca`, because
    /// there was nothing parseable to describe — which is a state D9 rotates rather than refuses.
    pub previous: Option<Ca>,

    /// Where this home and this machine stand now, measured after the grant.
    ///
    /// The same `CaStatus` `cert.ca_status` answers, produced by the same code. A rotation that did
    /// not commit reports the old authority here, because that is what is there.
    pub status: CaStatus,

    /// One entry per site, exactly `cert.issue`'s. Empty when nothing was committed.
    pub sites: Vec<SiteCertOutcome>,
}

#[non_exhaustive]
pub enum RotateOutcome {
    /// The authority was replaced and every store that could be reached was updated.
    Rotated {},

    /// Nothing was changed, and this is why — D7's fourth and fifth rows.
    NotCommitted { because: String },

    /// There is no authority to rotate — D9.
    NothingToRotate { because: String },
}

pub struct CaUninstallReport {
    /// What happened.
    pub outcome: UninstallOutcome,

    /// Where this machine stands now, measured after — D2. **What is left is read here and stated
    /// nowhere else**: `status.trust` is `Installed` when the system store still holds it, and
    /// `status.browsers` lists each database and whether it does.
    pub status: CaStatus,
}

#[non_exhaustive]
pub enum UninstallOutcome {
    /// Every store that could be reached no longer holds it.
    Removed {},

    /// Some store still does — D8's partial progress — and this is why.
    PartlyRemoved { because: String },

    /// This home has no authority to take out of anything.
    NothingToRemove { because: String },
}
```

An earlier draft of this section gave `CaUninstallReport` a `left: Vec<String>` beside `status`. It
was cut on the paragraph below: what is left is exactly what the measurement says, and a second
field restating it is a second answer that can disagree with the first. The outcome carries the
*reason*, which the measurement genuinely cannot.

`CaStatus` is reused rather than a new "where does trust stand" type invented beside it, so the two
cannot come to disagree about what reading a trust store means — the reasoning `Certificates::status`
already applies to `authority`.

**There is nowhere here a private key could travel**, which is how
`.claude/architecture/security-model.md`'s rule stays true: no type above has a field to put one in,
and `Ca` has carried only the public half since T48.

## D13 — The CLI

```
mix cert ca-rotate     [--yes] [--no-wait] [--json]
mix cert ca-uninstall  [--yes] [--no-wait] [--json]
```

`--yes` and `--no-wait` are the flags `mix doctor --repair` already has, for the same two reasons: a
script has nobody to answer the confirmation, and a caller that only wants the job started should
not be made to wait for a prompt.

Rendering follows `render::repair`: what happened, then what is left, then — when something is left
— the command to run next. `mix cert ca-status` for a rotation that did not commit,
`mix doctor --repair` for trust that could not be restored.

## What T54 deliberately does not do

- **No rotation on a timer.** The authority lives ten years, and this is an operation with a person
  on the other end of it, not something a scheduler does at three in the morning. T52's design says
  so in as many words and names this task.
- **No file is deleted** — D8. That is T87's, and it is the fifth task in a row to leave deletion
  alone on T42's D12 and T45's D13.
- **No second elevation prompt** — D4.
- **No `cert.ca_install`.** Installing already happens: at daemon start through
  `require_trust_store`, and on demand through `mix doctor --repair`. A third name for one operation
  is a third thing to keep in step.
- **No handshake.** Whether the padlock is green after a rotation is `mix cert status`, T53, and it
  is one command away.

## Testing

**Unit, over the decision that matters.** D7's four clauses are a free function over a `TrustState`
and a `bool`, tested as `problem` was in T53: store-holds-new, no-store, store-never-held-ours,
probe-failed, and the one that refuses.

**Unit, over staging.** A rotation whose probe says no leaves `certs/ca/root.crt` byte-identical to
what it was, and leaves no `certs/pending/` behind. This is the test the whole design exists for,
and it must assert both halves: a discard that deleted the staging *and* the live authority would
pass a test that only checked the staging.

**Integration, enumerating the stores** — `tls.md`'s acceptance criterion, against the mock host.
After `cert.ca_uninstall`, the trust store does not hold the DER and no browser database does. The
assertion carries a control: the same enumeration **before** the call finds it, so a test that
enumerated nothing cannot pass. That control is T52's lesson — an absence assertion passes just as
well when the log was never read.

**A rotation end to end is a system test, and finding that out cost a real certificate.**

This section first said the opposite. It planned an ordinary integration test asserting that a
rotation *changes nothing*, reasoning that no machine running `cargo test` can raise an elevation
prompt, so the grant would always fail and the discard path would always be the one taken.

**Measured on 2026-08-26, that is false.** Running the test on Windows raised a real UAC dialog in
the middle of `cargo test`, a person clicked Yes, and the run installed a certificate authority into
`LocalMachine\Root` — which rule 1 of `.claude/standards/testing.md` forbids in as many words, and
which no arrangement of the *home* can prevent, because the store a rotation reaches belongs to the
machine and not to the home.

So the end-to-end rotation is `#[ignore]`d and gated on `MIXENGINE_SYSTEM_TESTS=1`, and what it
asserts is the **invariant** rather than either outcome: a rotation either replaces the authority or
leaves it exactly as it was, and in neither case does it leave a candidate private key on disk.
Asserting one outcome would make the test a statement about whoever answered the prompt.

What is left running on an ordinary machine is the decision (`commits`, six unit tests), the discard
(`ca::discard` leaves the live pair byte-identical, in `mixengine-core`), and the two refusals that
never reach a store at all — a rotation with nobody to answer the confirmation, and a rotation on a
home with no authority. **The wiring between the decision and the discard is covered only by the
gated test**, and saying so is the honest accounting.

**End to end against a real Caddy, `#[ignore]`d.** `mix cert status` reports `Trusted` after a
rotation, on the machines that have a Caddy to run it against. This is the acceptance criterion
*"`mix cert ca-rotate` completes with all sites still trusted afterwards"*, and nothing short of a
handshake answers it.

No test touches a real trust store, a real NSS database or the real hosts file unless it is
`#[ignore]`d and gated on `MIXENGINE_SYSTEM_TESTS=1` — `.claude/standards/testing.md`, rule 1.

## Files

| File | What changes |
| --- | --- |
| `crates/mixengine-proto/src/cert_api.rs` | `CaRotateQuery`, `CaUninstallQuery`, `CaRotateReport`, `RotateOutcome`, `CaUninstallReport` |
| `crates/mixengine-proto/src/rpc.rs` | `CERT_CA_ROTATE`, `CERT_CA_UNINSTALL` |
| `crates/mixengine-core/src/certs/ca.rs` | `pending_root`, `promote`, `discard`; generation is `ensure` on that root |
| `crates/mixengine-daemon/src/elevation.rs` | `preflight` extracted; `grant_within` added |
| `crates/mixengine-daemon/src/certs/authority.rs` | new — both operations, taking what they need rather than living on `Certificates` |
| `crates/mixengine-daemon/src/certs.rs` | `pub(crate) mod authority;`, and the staging discard at start |
| `crates/mixengine-daemon/src/api/rpc.rs` | two routes |
| `crates/mixengine-cli/src/main.rs` | `CaRotate`, `CaUninstall` |
| `crates/mixengine-cli/src/render.rs` | `ca_rotate`, `ca_uninstall` |
| `.claude/features/tls.md`, `.claude/roadmap/phase-5-https.md` | as built |

The orchestration is its own module rather than a pair of methods on `Certificates`, because
`Certificates` holds a directory, a host and a store — and this needs the elevation queue, the job
registry and the service registry as well. `certs/renewal.rs` set that precedent in T52: the thing
that needs more than certificates takes them as arguments.
