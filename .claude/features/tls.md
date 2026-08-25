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

`cert.issue { domains }`:

- Leaf key ECDSA P-256, `serverAuth` EKU only, 90 days, SANs = the site's domains + `localhost`
  aliases where relevant. No wildcard for a public suffix; `*.blog.test` is allowed.
- Written with `0600` permissions, then the web server is reloaded (never restarted).
- Issuance is idempotent: an existing cert covering exactly the requested SANs with > 30 days left is
  reused.

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
| Firefox/Chrome on Linux | each NSS DB found under `~/.mozilla/firefox/*/` and `~/.pki/nssdb` | `certutil -A -d sql:<dir> -n MixEngine -t C,,` | `certutil -D` |

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
