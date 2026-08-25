# Automatic HTTPS

**Goal**: every site is `https://` with a green padlock in every browser, with no manual `openssl`
and no expiry surprises.

## Design

An internal CA (see [architecture/security-model.md](../architecture/security-model.md) for the
security constraints) issues short-lived per-site leaf certificates. We do **not** use ACME or
Let's Encrypt — local domains are not publicly resolvable and rate limits would bite.

```
certs/
  ca/root.crt  ca/root.key         ECDSA P-256, 10 years, pathlen:0
  sites/blog.test.crt / .key       90 days, SANs = every domain of the site
```

## First run

1. Generate the CA (`rcgen`).
2. Explain, in one screen, what installing it does and what it means.
3. Install into the OS trust store — **batched into the same elevation prompt** as the resolver
   config and the port grant, so first run costs one prompt in total. On Linux additionally into NSS DBs
   (`~/.pki/nssdb`, Firefox profiles) because Chrome and Firefox there do not read the system store.
4. Record `ca.installed_in_trust_store` and the fingerprint.

**Step 4 is not how T49a answers the question.** Nothing is recorded: `cert.ca_status` reads the
store every time it is asked, and the daemon reads it at every start. A stored flag would be a claim
about a machine that an operating-system update, another account, or a person with `certmgr` can
falsify without MixEngine hearing about it — and the read costs no privilege on any of the three
systems, which is what makes asking cheaper than remembering.

If the user declines, sites still work over HTTP; `https_enabled` is refused with a hint.

## Issuance

`cert.issue { site }` — **built in T50**:

- Leaf key ECDSA P-256, `serverAuth` EKU only, `digitalSignature` only, `IsCa::ExplicitNoCa`,
  90 days, `CN` = the primary domain, SANs = **exactly** the site's domains in the site's own order.
- The private key is written first, with the mode `mixengine_platform::write_private` gives it, then
  the certificate: a crash between the two leaves a state `leaf::read` can name.
- Issuance is idempotent, and it asks **four** questions before reusing what is there — see below.
- The front end serves it from **T51**, which is below.

## Serving it

**A site with a certificate has two addresses**, `http://` and `https://`, both serving the same
site. No redirect: a local webhook or an old client pointed at plaintext keeps working, and a POST
that follows a redirect only sometimes is a bug nobody attributes to their web server.

**A site whose certificate is missing is served over HTTP alone** rather than failing to render.
Validation judges a whole rendering, so one site with a `tls` line pointing at nothing would cost
every other site its configuration. `mix doctor`'s `SiteCertificateMissing` is where the gap is
reported, and it repairs without a prompt.

**Caddy renders two site blocks and nginx renders one `server`**, and the asymmetry is a fact about
the two programs rather than an inconsistency: Caddy attaches `tls` to a site block, so a block
naming both schemes is refused — `server listening on [:80] is HTTP, but attempts to configure TLS
connection policies` — while nginx attaches `ssl` to a `listen` line, so one block carries both.
Both were measured against the real program rather than reasoned about.

**Reload happens because the rendered file changed**, and it changes because its header carries the
certificate's fingerprint. A certificate is reissued to the same path, so nothing else about the
file would differ, the installer would find no change, and the running server would go on serving
the certificate it already holds in memory.

**From T51 a front end actually binds the TLS port.** It never did before, because no site had a
certificate. Both servers refuse the *whole* configuration when one listener cannot be bound, so on
a machine that has not been granted the ports the failure is not "no HTTPS" but "the reload was
refused and the old configuration is still running". The first-run grant covers 80 and 443 together,
so a machine that can bind one can bind the other — and `https_port` is a setting on both recipes so
that a test, or a person, can move it.

**T50 corrected three things this section said.** `cert.issue { domains }` would let a client decide
what a certificate covers, which is business logic in a client; the method names a **site**, and the
daemon reads that site's domains from its own rows. "SANs = the site's domains + `localhost` aliases
where relevant" is not implementable — nothing defines *relevant* — and a SAN added on MixEngine's
initiative would be in every certificate it ever issued, so the SAN list is exactly the site's
domains. And `*.blog.test` is **not** allowed: `domains::normalised` has refused wildcards since T44,
whose DNS server is what answers them, so no site row can hold one.

**Reuse asks a fourth question this section does not have**: was the certificate signed by the
authority this home has *now*. Without it, T54's rotation leaves every site holding a leaf that
parses, covers the right names and has eighty days left — and that no browser accepts. The
comparison is the leaf's issuer name against the authority's subject name, which is free because T48
put the key's identity into that name.

**And issuance runs before configuration is generated, never as part of it.** `.claude/CLAUDE.md`
says generated configuration is disposable and rebuilt from SQLite; a certificate is state that
cannot be rebuilt from a row, and throwing one away costs the trust of every browser holding a cached
chain. The daemon's start orders it — authority, trust stores, browsers, **certificates**, then the
generators — and `site.create` and `site.update` issue before their own walk.

## Renewal

- A daily scheduler task renews anything with **< 30 days** left, plus a check on daemon start
  (laptops are asleep more than they are awake — a pure timer is not enough).
- Renewal reissues, reloads, and emits `CertExpiring` only if renewal *failed*.
- Browsers reject certs longer than 398 days; even though these are private, staying at 90 days keeps
  us compatible with any future tightening.

## Trust store details

| OS | Store | Command / API | Removal |
| --- | --- | --- | --- |
| Windows | `LocalMachine\Root` | CryptoAPI (`CertAddEncodedCertificateToStore`) | `CertDeleteCertificateFromStore` |
| macOS | System keychain | `security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain` | `security delete-certificate -Z <the SHA-1 `security` reported>` — **not** `remove-trusted-cert`, which never returns |
| Linux | `/usr/local/share/ca-certificates/mixengine.crt` + `update-ca-certificates` (Debian) / `/etc/pki/ca-trust/source/anchors` + `update-ca-trust` (RHEL) | elevated file write | remove file + update |
| Firefox/Chrome on Linux | every NSS DB found under `~/.pki/nssdb`, `~/.mozilla/firefox/*/`, `~/snap/firefox/common/.mozilla/firefox/*/`, `~/snap/chromium/common/chromium/`, `~/.var/app/org.mozilla.firefox/.mozilla/firefox/*/` and `~/.var/app/com.google.Chrome/.pki/nssdb` — a directory counts when it holds `cert9.db` | `certutil -A -d sql:<dir> -n "MixEngine Local CA <key_id>" -t C,, -i <file>` | `certutil -D -d sql:<dir> -n "MixEngine Local CA <key_id>"` |

Detect the distro family by probing for the directories, not by parsing `/etc/os-release` version
strings.

**The `certutil` fallback on Windows was not built** — T49a, D6. This row used to name one; the API
is four calls, and spawning a process from a context holding an administrative token is a larger
surface than that, not a smaller one.

**A removal names an authority, never a certificate** — T49a, D5, and this row used to say "delete by
fingerprint". It cannot: a removal that could name an arbitrary certificate could take the root that
validates Windows Update out of a machine, through the audited helper and under the user's own Allow
click. What travels is the eight-character key-id from the CA's subject, and the helper removes only
certificates that carry it **and** pass the whole shape check an install has to pass.

**The macOS removal named here is not the one that was built**, and the difference was measured
rather than reasoned about. On a machine with no window server, `security remove-trusted-cert -d`
never returns — not under plain `sudo`, not under `sudo -H`, not with `HOME` unset, not against a
root-owned path, and not even when there is nothing left to remove. `trust-settings-import -d` hangs
the same way, while `trust-settings-export -d` reads that domain and `add-trusted-cert -d` writes
it: the admin trust domain there can be read and added to, and neither removed from nor replaced.

`security delete-certificate` answers at once and takes the trust setting out **with** the
certificate — the admin domain *is* `/Library/Keychains/System.keychain` rather than a store beside
it. It is targeted and not wholesale, proved by installing two certificates and deleting one: the
other was still there and still trusted. The certificate is named by the SHA-1 `security` itself
printed for it in the same listing the check ran against, so the DER remains what is checked and
nothing in the command comes from the request.

**The last row is T49b and the first three are T49a**, split at the privilege boundary: the system
stores need root and ride in the first-run elevation batch, while NSS databases belong to the user
and are written by the daemon with no prompt at all. T49b also starts from a measurement this table
does not have — on a stock Ubuntu 24.04, `certutil` is **not installed**; it ships in `libnss3-tools`.
A machine without it is a state to report, not a failure.

**T49b corrected three things this table said.** The nickname was `MixEngine`, under which two homes
on one machine overwrite each other's entry with no error; it now carries T48's key id, which is what
makes a removal precise. `~/.mozilla/firefox/*/` is where a *deb* Firefox keeps profiles — on Ubuntu
22.04 and later the `firefox` deb is a transitional package to the snap (`Version: 1:1snap1-0ubuntu5`,
`Pre-Depends: snapd`), whose profiles live under `~/snap`, so the two-root version of this row found
nothing on the distribution most people run and reported success. And the certificate goes in through
a **file**: measured, `certutil -A -i /dev/stdin` answers `SEC_ERROR_INVALID_ARGS` because it seeks
its input, and `certutil -A` with the PEM on stdin and no `-i` at all **exits 0 without installing
anything** — a silent success, which is the one outcome no caller can act on.

Nothing is created. A profile directory with no `cert9.db` has never been opened by its browser, and
a database MixEngine invented would be a file in somebody's home that no program asked for. The
legacy `cert8.db` format is not read either: Firefox has written `cert9.db` since version 58.

## Diagnostics

`mix cert status` shows, per site: cert present, days left, SANs match the
site's domains, CA installed in each store we know about, and — crucially — a live TLS handshake
against the site reporting the actual chain the browser will see. Most "padlock is broken" reports
are a stale cert after adding a domain; the SAN-mismatch check catches exactly that and offers
one-click reissue.

## Acceptance criteria

- New site → `https://blog.test` trusted in Chrome, Firefox, Safari and Edge on their respective
  platforms, with no browser restart beyond the first CA install.
- Adding a domain to an existing site reissues automatically and the padlock stays green.
- `mix cert ca-uninstall` leaves no MixEngine certificate in any store (verified by an integration
  test that enumerates the stores).
- `mix cert ca-rotate` completes with all sites still trusted afterwards.
