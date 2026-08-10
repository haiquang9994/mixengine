# Security model

MixEngine installs a root CA, edits the hosts file, opens listening ports and can expose a site to
the local network. Each of those is a footgun if done casually. This document is the contract.

## Privilege split

- **`mixengined` runs as the user.** It has no elevated rights and never asks for any except through
  the helper.
- **`mixengine-helper` is the only elevated component.** It is small, dependency-light, and its whole
  API is the closed `PrivilegedOp` enum in
  [platform-abstraction.md](platform-abstraction.md#privileged-operations).
- The helper **never** accepts a command, a path outside `MIXENGINE_HOME`, a script, or a
  certificate it did not verify. Every op is a typed struct; there is no `Exec { cmd }` variant and
  adding one requires an ADR.

### Helper request validation

For every request the helper:

1. Verifies the caller over the control channel — peer UID/SID must match the user that installed it
   (Unix: `SO_PEERCRED`; Windows: named-pipe impersonation + token SID compare).
2. Validates arguments structurally: domains match `^[a-z0-9-]+(\.[a-z0-9-]+)*$` and end in a
   configured managed TLD; paths are canonicalised and must be inside `MIXENGINE_HOME`; ports are in
   the allowlist `{80, 443, 53}` plus user-configured ones recorded at install time.
3. Refuses to touch any hosts-file line outside the `# BEGIN/END MixEngine` block.
4. Logs the operation with arguments to `logs/helper.log` (append-only, root-owned) — this is the
   audit trail `mix doctor` reads back.
5. Idles out after 5 minutes and exits; it is restarted on demand.

## Local CA

- Generated on first use with `rcgen`: ECDSA P-256, CN `MixEngine Local CA <short-fingerprint>`,
  **10-year** validity, `basicConstraints=CA:TRUE, pathlen:0`, `keyUsage=keyCertSign,cRLSign`.
- Private key is stored at `certs/ca/root.key`, mode `0600` (Windows: DACL current-user-only) and is
  **never** copied, exported by an RPC, or sent to a client. `cert.ca_status` returns the fingerprint
  and the public cert only.
- Leaf certs are constrained: 90-day validity, only the site's own domains as SANs, no wildcard for
  a public suffix, `extendedKeyUsage=serverAuth`.
- The user is told, in plain language, what installing the CA means, and `mix cert ca-uninstall`
  removes it from every trust store we touched. Uninstalling MixEngine removes it automatically.
- If the CA key is ever suspected leaked: `mix cert ca-rotate` generates a new CA, reissues all
  leaves and removes the old one from the trust stores. Ship this — a CA with no rotation path is
  worse than no CA.

## Network exposure

- Default bind for every service is `127.0.0.1`. Databases, caches and Mailpit stay loopback-only
  unless the user explicitly enables sharing per service.
- LAN sharing ([features/lan-sharing.md](../features/lan-sharing.md)) is **opt-in per site**, shows
  exactly which interface/IP will be exposed, is auto-revoked when the network changes (different
  SSID/subnet) and never applies to database ports — the GUI must refuse that combination.
- Generated DB instances get a random 32-char root password stored in the OS keyring, not a blank
  password. `mix service credentials <id>` reveals it on demand.

## Client authentication

- IPC socket/pipe permissions are the primary control (owner-only).
- The optional TCP listener requires `Authorization: Bearer <token>` from
  `run/api.token`; the token is regenerated on every daemon start.
- Extensions get their own scoped token and a declared permission set
  ([features/extensions.md](../features/extensions.md)); an extension cannot call `daemon.*` or
  `cert.*`.

## Supply chain

- Every downloaded runtime/package is verified against a SHA-256 pinned in the signed
  `packages.json` index; the index itself is verified with a minisign/Ed25519 public key compiled
  into the binary. A hash mismatch aborts and deletes the download.
- Downloads go over HTTPS with the system roots — **not** our own CA.
- Extension packages are verified the same way; unsigned extensions require an explicit
  `--allow-unsigned` and are marked as such in the GUI forever.

## What we explicitly do not defend against

Stated so nobody assumes otherwise: MixEngine is a *developer tool on a trusted single-user machine*.
It does not protect against a local attacker who already has the user's account — such an attacker
can edit `mixengine.db` and reach everything MixEngine can. Our goal is: no accidental exposure to
the network, no unreviewable privilege escalation path, and no residue left behind at uninstall.
