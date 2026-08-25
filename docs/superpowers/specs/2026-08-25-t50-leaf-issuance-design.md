# T50 — the leaf certificates, and the four questions "is this one still good" has to ask

T48 made an authority. T49a and T49b put it into every store on the machine that a browser reads.
Nothing has yet been signed by it, so no site has a certificate and `https://blog.test` has nothing
to present. This task is the signing: one leaf per site, ninety days, `serverAuth` only, reissued
when the site's names change.

It is also where three of `.claude/features/tls.md`'s sentences turn out to be stale, and where one
rule that file does not have turns out to be the one that matters after T54.

---

## D1. Issuance is a precondition of generation, never a part of it

The tempting design folds issuance into the configuration generator: it already walks every site as
[`Served`](../../crates/mixengine-core/src/generate/served.rs), it already knows which have
`https`, and doing both in one pass guarantees that a configuration referring to a certificate is
never written before that certificate exists — which is exactly T51's problem.

It is still the wrong place. `.claude/CLAUDE.md` states that generated configuration is
**disposable**: everything under `etc/` is regenerated from SQLite and never parsed back. A
certificate is the opposite — it is state, it cannot be reconstructed from the database, and
throwing one away costs the trust of every browser holding a cached chain. A generator that
sometimes produced state would make that rule unreadable.

So the ordering is stated instead of the coupling. At daemon start:

1. `certs::ca::ensure` — this home has an authority.
2. The trust-store batch (T49a) and the browser databases (T49b).
3. **Leaf issuance** — this task.
4. The configuration generator.

T51 inherits a guarantee rather than a mechanism: by the time a template is rendered, every
`https` site's certificate is already on disk or is already known to have failed.

## D2. `certs::leaf`, beside `certs::ca` and shaped like it

`crates/mixengine-core/src/certs/leaf.rs`, exporting the same four things `ca` does — the two paths,
`ensure`, and `read`. Pure: it takes a certificates directory, an authority, a set of domains and a
`SystemTime`, and it knows nothing about the daemon, the store or the wire.

That symmetry is worth having on purpose. Everything a reader has learned about `ca` — that damage
is reported and never repaired, that `read` is what a caller is told rather than a description of
what was just written, that the private key is written before the certificate — transfers.

**The dependency question is already settled.** `x509-parser` is a normal dependency of
`mixengine-core` and its manifest comment names this task: "the internal certificate authority (T48)
and, from T50, the leaves it signs". T49a hand-wrote a DER reader because that reader ran inside a
binary that runs as root, where twenty-two crates was the cost being refused. Nothing here runs as
root.

## D3. What a leaf is

| | |
| --- | --- |
| Key | ECDSA P-256, a fresh pair per certificate |
| Lifetime | 90 days |
| Extended key usage | `serverAuth`, and only that |
| Key usage | `digitalSignature`, and no `keyCertSign` |
| `basicConstraints` | not a CA |
| Subject | `CN=<primary domain>` |
| SANs | exactly the site's domains, in the site's own order |
| Permissions | the key through `mixengine_platform::write_private`; the certificate world-readable |

**Ninety days on a certificate nothing public will ever see.** `tls.md` gives the reason and it is
not caution for its own sake: browsers already refuse public certificates over 398 days, the
direction of travel is downwards, and a private CA that had drifted to ten-year leaves would
discover the next tightening as a support load rather than as a renewal.

**Signed through `Issuer::from_ca_cert_pem`**, whose documentation says plainly that it "will not
check for the presence of the `BasicConstraints` extension, or perform any other validation". So the
gate is not rcgen: it is `ca::read` returning `Present`, which already checks that the key and the
certificate are each other's. A caller holding anything else does not reach this module.

## D4. The SANs are the site's domains and nothing else

`tls.md` asks for "the site's domains + `localhost` aliases where relevant". The second half is not
implementable as written — nothing defines *relevant* — and the honest reading of it is worse than
the ambiguity: a SAN added on MixEngine's initiative is a SAN in **every** certificate it ever
issues, and `localhost` in particular would leave N certificates each claiming the same name, with
whichever one the front end happened to serve deciding the answer.

Somebody who wants `https://localhost` adds `localhost` as a domain of a site. That path already
exists, is visible in `mix site show`, and belongs to the person rather than to a default.

**And wildcards are not a case at all.** `tls.md` says "`*.blog.test` is allowed"; it is not, and has
not been since T44. `mixengine_core::domains::normalised` refuses a wildcard outright — its own test
says "a wildcard is T44's answer, not a row" — so no site record can hold one and no SAN list can
contain one. The line in `tls.md` is corrected rather than implemented.

**A file named after a domain is safe, and this was measured rather than assumed.** `normalised`
lowercases, restricts to `[a-z0-9.-]`, refuses IDN by name, refuses empty labels and spaces, and
requires a managed TLD — so a primary domain contains no path separator, no `*`, no `:`. The one
residual worry was Windows' reserved device names: a site at `nul.test` producing `nul.test.crt`.
Probed on Windows 11, `nul.test.crt`, `con.test.crt`, `com1.test.crt`, `aux.test.crt` and
`prn.test.crt` each wrote a real 32-byte file. The device rule applies to the stem before the final
extension — `nul.test` — and not to the first label. **No validation rule is added**, and this
paragraph exists so nobody adds one on the reasoning that was wrong.

## D5. Where a leaf lives

```
certs/
  ca/root.crt        ca/root.key
  sites/blog.test.crt   sites/blog.test.key
```

Named after the **primary domain**, as `tls.md` says, and the argument for it over the site's rowid
is that the directory describes itself. Somebody pointing another program at a certificate, or
answering "does this site have one", reads the listing; `certs/sites/7.crt` answers neither
question, and the configuration T51 generates would point at a path nobody can check by eye.

The cost is that changing a site's primary domain leaves the old pair behind. D10 says what happens
to it, which is nothing.

## D6. Reuse asks four questions, and the fourth is not in `tls.md`

An existing pair is reused — nothing is written, nothing is signed — when **all four** hold:

1. Both files are there, parse, and are each other's.
2. The certificate's SAN set **equals** the requested set. Not covers: equals. A certificate with a
   spare name is a certificate that keeps working after a domain was deliberately removed.
3. More than 30 days remain.
4. **It was signed by the authority this home has now.**

The fourth is the one that decides whether T54 works. Rotation replaces the authority; every leaf
signed by the old one still parses, still covers the right names, and still has eighty days left —
so a three-question reuse rule declares every site fine, and every browser rejects every one of
them.

**The check is the leaf's issuer name against the authority's subject name**, and it is free because
of a decision T48 already made: the CA's common name *is* `MixEngine Local CA <key_id>`, where the
key id is derived from the public key. So a string comparison of two distinguished names answers
"was this signed by the authority this home has now", with no extension parsing at all — and it gets
the rotation cases right in both directions. A rotation onto a **new** key changes the key id, so
every leaf is reissued; a rotation that re-signs the **same** key keeps it, and the leaves stay
valid, which is exactly what should happen.

`authorityKeyIdentifier` would answer the same question — rcgen writes both it and the subject key
identifier — and is not used, because it would mean reading an extension to learn something the
subject line already says out loud. That the two agree is worth a test rather than a second
mechanism.

Failing any of the four means issuing a fresh pair over the old one. Never a partial write: the key
is written first, exactly as `ca::ensure` does, so a crash between the two leaves a state `read`
recognises by name rather than one that looks like a certificate whose key was lost.

## D7. `cert.issue` names a site, never a list of domains

`tls.md` specifies `cert.issue { domains }`. That would put in the client the decision of what a
certificate covers, which is business logic, and `.claude/CLAUDE.md`'s first non-negotiable rule
says a client only renders what the daemon returns.

```rust
pub struct CertIssue {
    /// One site, or every site with HTTPS declared when it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site: Option<SiteRef>,
}

pub struct CertIssueReport {
    /// One entry per site considered, in primary-domain order.
    pub sites: Vec<SiteCertOutcome>,
}

pub struct SiteCertOutcome {
    /// The site, by its primary domain.
    pub domain: String,
    pub outcome: IssueOutcome,
    /// What is on disk afterwards.
    pub state: CertState,
}

#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IssueOutcome {
    /// A pair was written.
    Issued {},
    /// The four questions of D6 all answered yes.
    Reused {},
    /// Nothing was written and this is why — no authority, HTTPS not declared, no domains.
    Refused { because: String },
}
```

The absent-`site` form is what the producer itself calls and what `mix cert issue` runs with no
argument, so the automatic path and the manual one are the same code rather than two.

**No `force`.** The only thing it would buy today is reissuing a certificate that already passes all
four checks, which is a request nobody has. T53's one-click reissue follows a SAN mismatch — already
covered by question 2 — and T54's rotation is covered by question 4.

## D8. What a certificate is, on the wire

```rust
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CertState {
    Absent {},
    Present { cert: SiteCert },
    Unusable { because: Unusable },
}

pub struct SiteCert {
    pub subject: String,
    /// Every name it covers, as it covers them.
    pub sans: Vec<String>,
    /// SHA-256 of the DER, lowercase hex.
    pub fingerprint: String,
    pub not_before: Timestamp,
    pub not_after: Timestamp,
    /// Signed, and allowed to be negative — an expired certificate is a true state.
    pub days_left: i64,
}
```

`CaState`'s vocabulary, reused deliberately: `Unusable` is the same closed enum, because the ways a
key and a certificate on disk can disagree do not depend on which of the two they are.

**There is no `certificate_pem`.** `Ca` carries one because a client installs it; nothing installs a
leaf, so the field would be surface with no caller — and `.claude/architecture/security-model.md`'s
guarantee that no type can carry a private key is easier to keep on a type with fewer fields.

## D9. The producer, and its three triggers

The daemon's `Certificates` gains `issue(site: Option<SiteRef>)`, and it runs:

- **at start**, in the block T49a and T49b already occupy, immediately after the browsers and
  before the generator — D1's ordering;
- **after `site.create` and `site.update`**, because a site that just gained a domain has a
  certificate that fails D6's second question;
- **on `daemon.doctor_repair`**, through D11.

`site.delete` triggers nothing: there is no certificate to issue, and the one left behind is D10's.

Nothing here fails a start or a request. A home whose authority is `Absent` or `Unusable` gets
`Refused` with that reason for every site, which is a sentence `mix cert issue` prints and
`mix doctor` repeats — and a site over HTTP keeps working, which is what `tls.md` promises for a
user who declined the CA entirely.

## D10. Orphans are named and not removed

Changing a primary domain or deleting a site leaves `certs/sites/<old>.crt` behind.

T50 does not delete it. Removal is the direction that can do damage — T49a's D5, learned the
expensive way — and a sweep is a removal nobody requested, driven by a list that a failed database
read could make empty. What it would save is a few kilobytes of a certificate that expires by itself
inside ninety days, in a directory belonging to the user.

`mix cert status` (T53) can list what is on disk against what the sites say, and T87's cleanup is
the producer that removes it. This is T42's D12 and T45's D13, third time.

## D11. A doctor condition, and why this one is not T48's answer

`ProblemId::SiteCertificateMissing`, checked per site and repaired through `Planned::InHome` calling
the same `issue` the producer calls.

**T48 declined to add a condition and this adds one, and the difference is what the repair does.**
There, the condition was a damaged authority and the repair would have been to regenerate — throwing
away every leaf and every store that holds the old certificate, in answer to a request nobody made.
Here the repair is issuance: idempotent by D6, destructive of nothing, and needing no privilege at
all, which is why it is `InHome` and not a queue entry.

The check reports:

- every `https` site has a certificate passing D6 → `Ok`
- this home has no usable authority → `Skipped`, naming `mix cert ca-status`
- no site declares HTTPS → `Ok`
- one or more sites lack one → `Problem`, naming them

**It is a check on disk and not a handshake.** Whether a browser accepts what the front end actually
serves is a different and stronger claim, and it is T53's; the check's own wording says so, exactly
as T49a's trust-store check says it answers "is it in the store" rather than "does a browser trust
it".

## D12. Where the code lives

```
crates/mixengine-core/src/certs/leaf.rs      ensure, read, paths — the whole mechanism
crates/mixengine-core/src/certs/mod.rs       one line
crates/mixengine-proto/src/cert_api.rs       CertIssue, CertIssueReport, SiteCertOutcome,
                                             IssueOutcome, CertState, SiteCert
crates/mixengine-daemon/src/certs.rs         issue(), beside install_in_browsers()
crates/mixengine-daemon/src/api/rpc.rs       cert.issue
crates/mixengine-daemon/src/sites.rs         the trigger after create and update
crates/mixengine-daemon/src/main.rs          the trigger at start
crates/mixengine-daemon/src/doctor.rs        the check
crates/mixengine-daemon/src/repair.rs        InHome::IssueCertificates
crates/mixengine-cli/src/…                   mix cert issue, and its rendering
```

## D13. Testing

**The mechanism is unit-tested in core against a real authority**, not a mock: `ca::ensure` into a
temp directory costs one ECDSA key pair, and every question D6 asks is then a test that writes a
file and asks again.

- a first issue writes both halves, and the certificate has exactly the SANs asked for
- a second issue with the same domains writes nothing
- a second issue with one domain **added** writes a new pair
- a second issue with one domain **removed** writes a new pair — the case a "covers" rule passes
  and an "equals" rule catches
- a certificate with 20 days left is reissued; one with 40 is not
- a leaf signed by a *different* authority is reissued — D6's fourth question, and the one that
  makes T54 work. Built by generating a second CA into a second directory and signing against it,
  so the test exercises the same code path a rotation will
- and the leaf that was just issued carries an `authorityKeyIdentifier` matching the authority's
  subject key identifier, which is the assertion that keeps D6's cheaper check honest
- a key without its certificate, and a certificate without its key, each read as the `Unusable`
  variant that names them
- issuing against an `Absent` authority refuses and writes nothing

**And one end-to-end run through the CLI**, in `crates/mixengine-cli/tests/cert.rs`: create a site,
`mix cert issue`, assert `certs/sites/<domain>.crt` exists and `--json` reports `issued`; run it
again and assert `reused`. That is what proves the RPC, the daemon's wiring and the renderer agree,
which no core test can.

Nothing here needs a network, a browser, or a real web server. The handshake that does is T53's.

## D14. What this task does not do

- **No web-server wiring.** Nothing yet points nginx or Caddy at these files — T51.
- **No renewal schedule.** D6 reissues anything under 30 days *when asked*; asking on a timer is
  T52.
- **No handshake and no `mix cert status`** — T53.
- **No `force`, no rotation, no removal** — T53 and T54.
- **No orphan sweep** — D10.
- **No `localhost` or IP SANs** — D4.
