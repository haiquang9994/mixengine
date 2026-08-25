# T51 — web server TLS wiring

**Roadmap task:** T51, phase 5. **Depends on:** T50 (leaf issuance), T43 (the site generator).

T50 signs one certificate per HTTPS site and writes it under `certs/sites/`. Nothing reads it. This
task is the reading: a site that declares HTTPS is served over TLS by whichever front end this home
runs, with the leaf its own authority signed.

Two things this task was expected to do turn out to be already done, and one thing it was not
expected to do turns out to be necessary. Both are below, with the measurements.

---

## D1 — `auto_https off` stays exactly as it is

**Measured, not assumed.** Caddy 2.11.4 — the version the CI leg fetches — was run against a config
with `auto_https off`, a site naming `https://` and a `tls` directive pointing at real files. It
validates, and it serves TLS. The same is true of `disable_certs`.

So the roadmap's "**disable Caddy's automatic ACME** explicitly" was discharged by T43, which made
`auto_https` a setting with `off` as its preset and wrote down why. There is nothing to change here,
and changing it to `disable_certs` for the look of the thing would be motion without a reason.

What T51 owes this line is a **test** rather than an edit: an assertion that the generated global
block still says `off` while sites are being served over TLS, so that a later change to the preset
cannot quietly re-enable a public certificate request for a name that resolves nowhere.

## D2 — an HTTPS site renders **two** site blocks, not one block with two schemes

**This is the design's first draft being wrong, caught by running Caddy.** The obvious rendering is
one block naming both schemes:

```caddyfile
http://blog.test, https://blog.test {
	tls `…/blog.test.crt` `…/blog.test.key`
	…
}
```

Caddy refuses it:

```
adapting config using caddyfile: server listening on [:80] is HTTP, but attempts to configure
TLS connection policies
```

A `tls` directive inside a block applies to every listener that block produces, and one of them is
plaintext. The rendering that works is two blocks over the same document root:

```caddyfile
http://blog.test, http://www.blog.test {
	root * `C:/Users/someone/blog`
	file_server
}

https://blog.test, https://www.blog.test {
	root * `C:/Users/someone/blog`
	tls `C:/Users/someone/.mixengine/certs/sites/blog.test.crt` `…/blog.test.key`
	file_server
}
```

Every domain appears twice, once per scheme, because a bare `blog.test` means HTTPS to Caddy — which
is why `site.caddy` already writes `http://` today and says so in its own header.

The handler stanza — `php_fastcgi`, `file_server`, `reverse_proxy` — is identical in both, which is
the cost: it is written twice per HTTPS site. A Caddy snippet (`(name) { … }` plus `import name`)
would remove the duplication and is **not** taken: a snippet is defined in the Caddyfile and
imported by the site files, which puts one site's handler in the shared document that
`.claude/features/` D-notes and `caddy.rs`' own header say must stay empty of per-site content —
the whole reason `sites/*.caddy` is one file per site is that a broken site takes only itself down.
Repeating four lines is cheaper than reintroducing a shared file every site depends on.

## D3 — the generator reads the disk, in exactly one place

`crate::generate` renders rows into text and touches no disk. `tls` names two files, so something
has to know whether they are there. The choice is where.

**In `Served`, when it is built from the row** — `generate::served`, which already turns a
`SiteRecord` into what a template sees. It gains:

```rust
/// The certificate this site is served with, when it has one — T51.
pub certificate: Option<SiteCertificate>,

pub struct SiteCertificate {
    /// Absolute path to the certificate, as the template must write it.
    pub certificate: PathBuf,
    /// Absolute path to the private key.
    pub key: PathBuf,
    /// SHA-256 of the certificate DER, lowercase hex — see D5.
    pub fingerprint: String,
}
```

filled by `certs::leaf::read`, which T50 already wrote and which answers `Present` only when both
halves are there, parse, and are each other's. **Templates stay pure**: they see an `Option` and
branch on it, exactly as they branch on `kind` today.

This is a real change to what the module is: `generate` is no longer a function of the database
alone. That is stated here rather than discovered later, and it is bounded — one call, in one
constructor, whose result is data the rest of the render treats like any other field.

**The cost is a parse per site per render.** `leaf::read` parses X.509 rather than calling `stat`,
because D5 needs the fingerprint and because "the file exists" is a weaker claim than "the pair on
disk is usable" — a truncated certificate would pass a `stat` and fail `caddy validate`, which is
the failure mode this whole decision exists to avoid.

## D4 — a missing certificate is HTTP-only, never a failed render

A site whose `leaf::read` is anything but `Present` renders its plaintext block and no TLS block. It
keeps working over HTTP; the other sites are untouched; `mix doctor`'s `SiteCertificateMissing`
(T50) already reports it and already repairs it without a prompt.

**The alternative is worse than it looks.** Rendering `tls` at a path that is not there fails
`caddy validate`, and validation judges the whole staged rendering — so one site with no certificate
would cost every site its new configuration. `caddy.rs`' header says the one-file-per-site layout
exists precisely so "a site whose configuration is broken fails validation on its own rather than
taking the other twelve down with it", and a `tls` line pointing at nothing would defeat it from
inside.

This is also why D3 uses `leaf::read` rather than `Path::exists`: the check that decides whether to
write a `tls` line should be the same check that decides whether the pair is usable.

## D5 — the fingerprint in the header is what makes a reload happen

**The bug this prevents is silent and would have shipped.** T50 reissues into the *same* path. So
after `mix site update --domain` adds a name and T50 signs a new certificate covering it, the
rendered site file is **byte-identical** to the one already installed. `document::install` compares
before it stages, finds no difference, skips validation and reloads nothing. The running server goes
on serving the old certificate from memory, and the browser reports a name mismatch for a
certificate that is, on disk, perfectly correct.

The fix is to make the rendering depend on *which* certificate it is, not just on its path. The
generated header gains a line carrying the fingerprint `leaf::read` already returns:

```
# Certificate sha256:3f2a…  — not a note. This line exists so that this file differs when the
# certificate differs, which is what makes the server reload. Delete it and a reissued certificate
# is never served.
```

**The wording is part of the design.** The failure mode of a stamp that looks like a comment is
somebody tidying it away, and nothing saying so until a padlock goes red months later. It says
outright that it is a mechanism.

**And two tests guard it**, in `mixengine-core`: reissuing for the same site makes the rendered file
differ, and rendering twice with nothing changed makes it identical. The second half matters as much
as the first — it catches a rendering that has accidentally become unstable, which would reload the
front end on every unrelated `service.*` call. Between them, deleting the line turns a test red whose
name says why, instead of breaking a user's padlock in silence.

**Why not reload explicitly instead**, from whichever code just signed a certificate — which reads
better, since "the certificate changed, so tell the server" is the actual causality. Because it
requires every such place to remember, and there will be more of them: three today (start, site
create and update, `doctor --repair`), plus T52's scheduler and T54's rotation. Forgetting one fails
silently, which is the exact failure this decision exists to prevent. The fingerprint is read from
the certificate on every render, so it is already correct for code that has not been written yet.

**Why not put the fingerprint in the certificate's filename**, which would make the path itself the
thing that changes — no stamp needed. Because it accumulates superseded files that nothing deletes
(T50 and T51 both decline to remove anything), and because it overturns T50's D5, which settled the
filename on the primary domain a week ago. Reversing a merged decision to avoid one line is a poor
trade.

**Why not teach `document::install` about external inputs**, which is the most general answer and
the one that belongs somewhere: any generated file pointing at another file has this problem.
Because deciding by comparison means remembering the inputs' previous hashes, which means new state
on disk — and the cheapest way to remember an input's hash is to put it *in the output*, which is
this decision. It is not a workaround for the general mechanism; it is how a diff-driven system
notices that an input moved.

**A modification time would be cheaper and is wrong**: an mtime changes when a file is copied or
restored and does not change when two homes hold different certificates at the same instant, so it
would both reload for nothing and fail to reload when it mattered.

## D6 — nginx gets the same treatment

`Served.https` would otherwise be honoured by one recipe and silently ignored by the other, and M5
promises a padlock rather than a padlock on Caddy. `nginx/site.conf` gains, inside the same `server`
block it already renders:

```nginx
    listen {{ listen_tls }} ssl;
    ssl_certificate "{{ certificate }}";
    ssl_certificate_key "{{ key }}";
```

Quoted and forward-slashed, on the escape rule that template's own header states.

**One `server` block rather than two is the expectation, and it is not yet measured.** nginx attaches
`ssl` to a `listen` line rather than to the block, so one block should carry both listeners without
the conflict D2 hit — but that is the same shape of reasoning D2 disproved for Caddy, and it is worth
no more here for being about a different program. **The plan's first nginx step is `nginx -t` against
a rendering that carries both listen lines**, before any template is written around the assumption.
If nginx refuses, it takes Caddy's two-block shape and the two recipes diverge for a measured reason
rather than a guessed one.

No `ssl_protocols`, no cipher list, no HSTS: nginx's own defaults are current, and a hand-written
cipher list is a thing that rots in a repository until it is the reason a browser refuses.

## D7 — the TLS port, and the machine that moves it

Caddy needs nothing: `https_port` is already in the global block and already passes through the
`bound` filter that maps 443 to 8443 on macOS.

nginx has no global listen, so the per-site line carries it — `listening(bind, bound(https_port))`,
beside the `listening(bind, bound(port))` already there. The same function, the same filter, a
second call: the mapping stays in one place, and `mixengine-platform` remains the only thing that
knows which system moves the port.

## D8 — what has to be true, and how it is proved

**Unit, in `mixengine-core`:** an HTTPS site with a certificate renders two Caddy blocks and exactly
one `tls`; the same site with no certificate renders one block and no `tls`; a site with
`https_enabled = false` renders one block whatever is on disk; the nginx rendering carries two
`listen` lines and two `ssl_` directives; the header's fingerprint line changes when the certificate
does, and **does not** change when nothing does — the second half is the assertion that catches a
render made accidentally unstable, which would reload the front end on every unrelated call.

**Against the real program**, in `crates/mixengine-cli/tests/caddy.rs` and its nginx counterpart —
`#[ignore]`d, fetched by CI, the only place a template is judged by the thing that reads it: a home
with an HTTPS site generates a configuration the server **accepts**, and a request over TLS to the
site's own name returns the site. That is the assertion that would have caught D2, and none of the
unit tests above could have.

## D9 — what this deliberately does not do

**No redirect.** `http://blog.test` keeps serving the site rather than answering 308. A local
webhook or an old client pointed at plaintext keeps working, and a POST that follows a redirect only
sometimes is a bug nobody would attribute to their web server's configuration. A site has two real
addresses, which is stated in the generated comment rather than left to be discovered.

**No renewal, no scheduler** — T52. This renders whatever T50 last wrote.

**No handshake check.** Whether the running server actually presents this certificate is `mix cert
status`, T53. What is proved here is that the configuration is accepted and the file is served.

**No HSTS, no OCSP stapling, no TLS version pinning.** Every one of them is a line that ages badly
in a template, and none is needed to make a local site trusted.

**Nothing is deleted.** A site that stops declaring HTTPS loses its TLS block on the next render;
its certificate stays on disk. Removal is T54's, on the reasoning T42's D12 and T45's D13 set and
T50 followed.
