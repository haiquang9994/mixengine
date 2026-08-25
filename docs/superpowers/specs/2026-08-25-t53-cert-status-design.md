# T53 — `mix cert status`, with a live handshake

**Roadmap task:** T53, phase 5. **Depends on:** T48 (the authority), T50 (leaf issuance), T51 (the
front end serves the leaf), T49a/T49b (the trust stores).

Everything phase 5 has built so far reads files. T48 reads the authority off disk, T50 writes a leaf
and reads it back, T51 renders a `tls` line naming it, T52 replaces it before it expires — and every
one of those is a claim about a *file*. Not one of them establishes that the server running on this
machine is presenting that file to anything.

That gap is exactly where the reports come from. `.claude/features/tls.md` names it: *"most 'padlock
is broken' reports are a stale cert after adding a domain"* — a case where the file on disk is
already correct, `mix doctor` is green, and the browser still refuses, because the running server
holds the old certificate in memory.

This task is the first measurement in the repository that answers **"is the padlock green"** rather
than inferring it. It is the same discipline T49b's browser probes arrived at the hard way: a
certificate list is not an answer, only a handshake is.

---

## D1 — three of the four things asked for already exist

`tls.md` asks `mix cert status` to show, per site: *cert present, days left, SANs match the site's
domains, CA installed in each store we know about, and — crucially — a live TLS handshake*.

- **Cert present, days left** — `mixengine_core::certs::leaf::read` returns exactly that, as
  `CertState`, and has since T50.
- **SANs match the site's domains** — `mix doctor`'s `SiteCertificateMissing` check already
  compares them.
- **CA installed in each store** — `cert.ca_status` answers it, per store and per browser database,
  since T49a and T49b.

So T53 is mostly an **assembly**, and one genuinely new capability: the handshake. The assembly is
still worth building — the three answers live in three different commands today, and the question a
person actually has ("why is my padlock red") is answered by their conjunction.

**And one thing asked for turns out to be already built too.** *"Offers one-click reissue"* — `mix
cert issue --site blog.test` reissues one site, and `mix doctor --repair` reissues every site with
no prompt. T53 adds no new mechanism for it; what it adds is the client naming the command when the
condition calls for it, which is D5.

## D2 — the handshake connects to loopback and sends the site's name

A browser resolves the name first. This does not, and the reason is attribution.

Whether `blog.test` resolves is a separate question with a separate answer already: `mix doctor`'s
`DomainUnreachable`, and behind it the resolver wiring of T45 and the hosts file of T42. A handshake
that resolved the name would report *"TLS failed"* on a machine whose only problem is that no
resolver was ever wired — a diagnostic that sends whoever reads it to the wrong half of the system.

So the connection is to `127.0.0.1:<https_port>` with SNI set to the site's primary domain, and the
answer says so in as many words. What it measures is the front end: the certificate the running
server presents for that name.

**The consequence is stated rather than hidden.** A green answer here plus a red padlock in a
browser means the name does not resolve to this machine, and that is `mix doctor`'s to say.

## D3 — one connection, and a verifier that both captures and judges

rustls lets a client install its own `ServerCertVerifier`. This one does two things and returns
`Ok` either way, so that the connection completes and the presented chain can be read:

1. **Captures** the chain the server sent.
2. **Judges** it, by delegating to a real `WebPkiServerVerifier` whose only root is this home's own
   authority, and records that verdict.

What it answers with:

```rust
pub enum Handshake {
    /// This site declares no HTTPS, so nothing was attempted.
    NotAsked {},

    /// Nothing answered, and this is why: no front end in this home, or a port nothing is
    /// listening on.
    NotServed { because: String },

    /// Something answered and TLS did not complete.
    Failed { because: String },

    /// A chain was presented.
    Presented {
        /// Its leaf, described exactly as the file on disk is — see D4.
        cert: SiteCert,
        /// Whether it validates against this home's authority.
        trust: Verdict,
    },
}

pub enum Verdict {
    Trusted {},
    Rejected { because: String },
}
```

**A `Verdict` rather than a `bool` and a reason beside it**, on `CaState`'s shape: a boolean with an
optional sentence has a fourth state nobody means — rejected with no reason, trusted with one — and
every branch that can say why is required to.

**Rejected: connect permissively and compare issuer names afterwards.** That is the cheap rule
`leaf::reusable` uses for a different question, and it is not verification: a chain carrying the
right issuer name and the wrong key would be reported as trusted by the one command a person types
in order to ask whether their certificate is genuinely trusted. Cheapness is the wrong axis for
this particular answer.

**Rejected: two connections, one strict and one permissive.** It costs a second handshake and opens
a gap — a reload between them, which T52 can now cause on its own, would make the two answers
describe two different servers. One connection cannot disagree with itself.

## D4 — what the presented certificate is compared against

Two comparisons, and neither is the one a file-reading check already makes:

1. **The site's declared domains against the SANs of the certificate on disk.** What this catches is
   a certificate that cannot cover the names whatever its state — and the fix is a reissue, because
   no server can serve a name nothing has signed.
2. **The certificate the server presented against the one on disk, by fingerprint.** What this
   catches is the failure `tls.md` names: a server holding a certificate in memory that the file
   beside it no longer matches. The fix is a reload, and it is a different fix, which is why it is a
   different condition.

**By fingerprint and not by names**, for the second. A hash differs whenever anything differs — a
renewal, a rotation, a reissue with the same names — where a name comparison would call a server
holding last month's certificate correct as long as the names had not changed. It is also the field
T50 already computes on both sides, so the two are hashed the same way by construction.

A command comparing the file to the row would find them in agreement and report that everything is
fine, which is exactly the report this task exists to stop.

The file is still reported, because a person needs both halves to know which way it broke. `SiteCert` —
the type `CertState::Present` already carries — describes both, so "the file and the wire differ" is
visible by comparison rather than needing a type of its own to express.

## D5 — the daemon names the condition; the client names the command

The answer carries a closed enum rather than advice:

```rust
pub enum CertProblem {
    /// No usable certificate on disk for this site.
    NoCertificate,
    /// There is one and it is inside the renewal window.
    Expiring,
    /// The certificate on disk does not cover the names this site declares.
    NamesDiffer,
    /// Nothing is serving TLS for this site.
    NotServed,
    /// The server is presenting a certificate other than the one on disk.
    ServedCertificateDiffers,
    /// The presented chain does not validate against this home's authority.
    NotTrusted,
}
```

**This is `ProblemId`'s decision, applied again.** That type's own documentation says it is *"a name
for a condition rather than advice"*, and the reason holds here unchanged: `mix` renders `mix cert
issue --site blog.test`, and a graphical client renders a button. A daemon that returned the command
string would be sending a GUI a sentence telling its user to open a terminal.

It also keeps the choice reversible. The command's name is the client's to change.

**Not `ProblemId` itself.** These are conditions of one site's certificate, reported by a status
command; `ProblemId` names conditions of the machine that `mix doctor` repairs. Merging them would
mean every one of these needs a repair in `repair.rs`, and `ServedCertificateDiffers` is repaired by
reloading a front end rather than by touching a certificate at all.

## D6 — the port comes from the settings the rendering used

The front end's TLS port is `https_port`, a setting T51 made movable on both recipes. The daemon
needs the number, and the rule that decides where it comes from is `.claude/CLAUDE.md`'s: generated
configuration is disposable and is never parsed back into state.

`mixengine_core::generate::Generated` — what the generator hands back per service — carries the
spec, the files it wrote and its first-run step, but not the `Settings` it merged in order to render
them. T53 exposes that. The daemon then reads **the same value the template read**, from the same
merge, rather than repeating `Settings::merge` at a second site that can drift from the first.

**Rejected: declaring 80 and 443 in the front end's `ServiceSpec::ports`.** That field is what T38
diagnoses a failed start against, and the recipes deliberately declare the admin endpoint alone —
with a comment explaining that Caddy binds neither port until a site asks it to. T51 has made that
comment stale and it is worth revisiting, but making the change here would have T53 alter what a
failed start is diagnosed against in order to read one number. Separate work, separate task.

## D7 — a new edge and not a new package

`tokio-rustls` and `rustls` become direct dependencies of `mixengine-daemon` — the first for the
connector, the second for the verifier trait and the root store the verifier is built from. Both are
in the tree already, through `reqwest`, so nothing new is downloaded, audited or shipped: the same
argument `Cargo.toml` makes for `rcgen` taking `ring` from `rustls`. The version each is pinned at
is the one already resolved, so the tree carries one copy and not two — which is the condition that
makes "a new edge" true rather than merely stated.

The alternative was to make an HTTP request through `reqwest` and read the certificate out of it.
That reaches only the leaf rather than the chain, requires the site to answer a request rather than
merely complete a handshake, and would give the daemon an outbound HTTP client for a connection to
itself. `mixengine-supervisor`'s own note already refuses an `https://` health check for a related
reason: what serves the local socket wants no TLS stack.

## D8 — what has to be true, and how it is proved

**Unit, in the daemon crate**: a rustls server is started inside the test on a loopback port,
presenting a certificate this test generated, and the handshake reports its fingerprint, its SANs
and a verdict; a port nothing is listening on is `Handshake::NotServed` rather than an error; and a
server presenting a certificate signed by **another authority** is `Presented` with
`Verdict::Rejected` — the case D3's rejected alternative would have called trusted, and the reason
this test is worth more than the two beside it.

**End to end, in `crates/mixengine-cli/tests/caddy.rs`** — where T51 already runs a real Caddy
serving a real site over TLS: `mix cert status` reports a handshake that is `Presented` and
`trusted`. That assertion is the first in this repository to measure a green padlock rather than
infer one, and no unit test can reach it: what it proves is that the certificate MixEngine wrote,
the configuration MixEngine rendered and the server MixEngine started agree with each other.

**And the case the task was named for**: a site whose certificate is replaced on disk while the
front end is running, without a reload, reports `ServedCertificateDiffers`. That is the "padlock
broke after adding a domain" report, reproduced deliberately.

## D9 — what this deliberately does not do

**No name resolution** — D2.

**No `--fix` flag.** `mix cert issue --site` and `mix doctor --repair` both already reissue, and a
diagnostic command that also repairs is a command doing two jobs. What T53 adds is the condition
that tells a client which command to offer.

**No change to `ServiceSpec::ports`** — D6.

**No rotation and no removal.** `cert.ca_rotate` and `ca_uninstall` are T54.

**No `mix doctor` check.** Adding a `ProblemId` means deciding what repairing it is, and the answer
for `ServedCertificateDiffers` is "reload the front end" — a repair that belongs with whatever task
decides the front end may be reloaded on a diagnosis. T48 declined a check for the same reason and
said so.

**Nothing is written.** `cert.status` reads: it opens a socket, reads two files and closes both. A
status command that reissued would be the sixth time this phase had to argue T42's D12, and the
first time it lost.
