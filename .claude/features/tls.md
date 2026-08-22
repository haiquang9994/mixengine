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
| Windows | `LocalMachine\Root` | CryptoAPI (`CertAddEncodedCertificateToStore`), `certutil` fallback | delete by fingerprint |
| macOS | System keychain | `security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain` | `security delete-certificate -Z <sha1>` |
| Linux | `/usr/local/share/ca-certificates/mixengine.crt` + `update-ca-certificates` (Debian) / `/etc/pki/ca-trust/source/anchors` + `update-ca-trust` (RHEL) | elevated file write | remove file + update |
| Firefox/Chrome on Linux | each NSS DB found under `~/.mozilla/firefox/*/` and `~/.pki/nssdb` | `certutil -A -d sql:<dir> -n MixEngine -t C,,` | `certutil -D` |

Detect the distro family by probing for the directories, not by parsing `/etc/os-release` version
strings.

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
