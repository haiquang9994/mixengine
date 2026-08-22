# Security model

MixEngine installs a root CA, edits the hosts file, opens listening ports and can expose a site to
the local network. Each of those is a footgun if done casually. This document is the contract.

## Privilege split

**No MixEngine process runs as root between operations.** See
[../decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md).

- **`mixengined`, the CLI, the GUI and every managed service run as the user.**
- **`mixengine-elevate` is the only elevated component**, and it exists for seconds at a time: the
  daemon spawns it through the OS elevation prompt (UAC / osascript / pkexec), it performs one
  batch of operations, and it exits. It has no listener, no service registration, no idle state.
- Its whole API is the closed `PrivilegedOp` enum in
  [platform-abstraction.md](platform-abstraction.md#privileged-operations). It **never** accepts a
  command, a script, an arbitrary path, or a certificate it did not verify. There is no
  `Exec { cmd }` variant, and adding one requires an ADR.

### Request validation

The daemon runs as the user. **If the daemon is compromised, it is the attacker** — so the elevated
process validates everything again rather than trusting its caller:

1. Parses a typed request; anything unparseable is rejected without partial effect.
2. Domains must match `^[a-z0-9-]+(\.[a-z0-9-]+)*$` and end in a configured managed TLD. Paths are
   canonicalised and must resolve inside `MIXENGINE_HOME`. Ports must be in the recorded allowlist.
3. Refuses to touch any hosts-file line outside the `# BEGIN/END MixEngine` block.
4. Writes atomically under an advisory lock: temp file in the same directory → fsync → rename
   (`ReplaceFile` on Windows, to preserve ACLs).
5. Appends one JSON line per operation to a root-owned log **outside `MIXENGINE_HOME`** —
   `%ProgramData%\MixEngine\elevate.log`, `/Library/Logs/MixEngine/elevate.log`,
   `/var/log/mixengine/elevate.log` — created by the helper on first run, never by an atomic replace.
   Inside `MIXENGINE_HOME` "append-only" would be a promise the filesystem does not keep: a
   root-owned file in a user-owned directory can be renamed or unlinked by that user. It is the audit
   trail `mix doctor` reads back, **and it makes what ran readable, nothing more** — it prevents
   nothing, and specifically not the binary-replacement path below, since a helper that has been
   replaced is also the thing writing the log.
6. Exits. A distinct exit code reports "user declined", which the daemon treats as a normal outcome.
7. Runs every external program as an **argument vector**, never through a shell and never with a
   command line it interpolated. The sharpest edge is the launcher rather than the helper: on macOS
   the prompt is raised by `do shell script … with administrator privileges`, which takes a *string*,
   and a quoting mistake in the path interpolated into it is arbitrary code as root (T40a).

### Elevation budget

Every prompt is a cost, so pending operations are queued and flushed in a **single** invocation.
Elevating inside a loop is a defect. Expected lifetime total: one prompt at first run (CA + resolver
+ port redirect, batched), one when the user first enables LAN sharing, one at uninstall. **Creating
a site prompts for nothing** — that is a requirement, not an aspiration, and it is why the internal
DNS server is the primary domain mechanism ([../features/domains-and-dns.md](../features/domains-and-dns.md)).

### Auto-update boundary

`mixengine-elevate` is **excluded from auto-update**. It is installed once to a root-owned directory
and replaced only through its own explicit elevation prompt, with a minisign check performed inside
the elevated context. An auto-updated binary that runs as root, with no OS code signature, is a local
privilege-escalation vector — see [../features/updates.md](../features/updates.md).

## Local CA

- Generated on first use with `rcgen`: ECDSA P-256, CN `MixEngine Local CA <short-fingerprint>`,
  **10-year** validity, `basicConstraints=CA:TRUE, pathlen:0`, `keyUsage=keyCertSign,cRLSign`.
- Private key is stored at `certs/ca/root.key`, mode `0600` (Windows: DACL current-user-only) and is
  **never** copied, exported by an RPC, or sent to a client. `cert.ca_status` returns the fingerprint
  and the public cert only. The directory it sits in is closed off first, by `DirectoryAccess` at
  bootstrap — a key written `0600` into a `0755` directory is still listed by everyone, and on
  Windows a `certs/` that inherited `C:\` is readable by every local account.
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
can edit `mixengine.db` and reach everything MixEngine can.

Specifically: if `mixengine-elevate` is installed somewhere the user can write, malware running as
the user could replace it and gain root the next time the user approves a prompt. We reduce this by
installing it to a root-owned location and keeping it out of the auto-update path, but we do not
claim to eliminate it — it is the same trust model as `sudo` on a personal machine.

Our goal is: no accidental exposure to the network, **no process holding root while idle**, no
unreviewable privilege-escalation path, and no residue left behind at uninstall.
