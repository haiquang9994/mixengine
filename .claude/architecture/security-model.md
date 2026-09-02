# Security model

MixEngine installs a root CA, edits the hosts file, opens listening ports and can expose a site to
the local network. Each of those is a footgun if done casually. This document is the contract.

## Privilege split

**No MixEngine process runs as root between operations.** See
[../decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md).

- **`mixengined`, every client and every managed service run as the user.**
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

- Generated **when the daemon starts** with `rcgen`: ECDSA P-256, CN `MixEngine Local CA <key-id>`,
  **10-year** validity, `basicConstraints=CA:TRUE, pathlen:0`, `keyUsage=keyCertSign,cRLSign`, and
  no subject alternative name at all — an authority is not a server, and a name on one invites
  something to accept it as a leaf.
- **`<key-id>` is the first 8 hex characters of the SHA-256 of the public key, and not of the
  certificate.** This line used to say `<short-fingerprint>`, which cannot exist: a fingerprint is a
  hash *of the certificate* and the subject is inside the bytes being hashed, so no ordering
  produces it. Deriving it from the key is also the more useful of the two, because it survives
  re-signing the same key and therefore makes two certificates for one authority recognisable as
  one. `cert.ca_status` still reports the certificate's own SHA-256 as the **fingerprint**, since
  that is what a browser shows and the only value a person can compare against anything.
- **At start rather than on first use**, so that the trust-store install falls inside the same single
  first-run elevation batch as the resolver wiring and the port grant. An authority that first
  appeared when somebody created an HTTPS site would put that install in a second batch and
  therefore behind a second prompt — which is the promise three lines above this one. T45 reached
  the same conclusion for the resolver first, and for the same reason.
- **A damaged authority is reported and never silently replaced.** Regenerating would invalidate
  every leaf already issued and every trust store holding the old certificate, in answer to a
  request nobody made; `cert.ca_status` names which way it is damaged, and `mix cert ca-rotate` is
  the command that has the steps a replacement needs.
- Private key is stored at `certs/ca/root.key`, mode `0600` (Windows: DACL current-user-only) and is
  **never** copied, exported by an RPC, or sent to a client. `cert.ca_status` returns the fingerprint
  and the public cert only, and there is no field on any of its types a key could travel in.
- **The key is protected twice, and neither half is redundant.** The directory it sits in is closed
  off first, by `DirectoryAccess` at bootstrap — a key written `0600` into a `0755` directory is
  still listed by everyone, and on Windows a `certs/` that inherited `C:\` is readable by every
  local account. And the file carries its own permission, applied by `write_private` **as it is
  created** rather than after: on Unix the mode is an argument to `open(2)`, and on Windows the file
  is made empty, restricted, and only then written. Relying on the directory alone would make the
  key's protection a property of something `mix doctor` already has a name for losing
  (`HomePermissionsLost`); applying the permission afterwards would leave an instant in which the
  key existed at whatever the umask handed out.
- Leaf certs are constrained: 90-day validity, only the site's own domains as SANs, no wildcard for
  a public suffix, `extendedKeyUsage=serverAuth`.
- **The key is protected per-user and the trust is granted machine-wide, and that asymmetry is
  deliberate.** The private key is one account's (`0600`, a DACL naming the current user); the
  certificate goes into `LocalMachine\Root` and the System keychain, which every account on the
  machine reads. On a machine with more than one person that means account B's browser trusts an
  authority whose key lives in account A's home, and account A can mint a certificate for any name.
  That is inside the trust model stated at the foot of this document — a developer tool on a trusted
  single-user machine — but nobody had said so about this specific pair, so it is said here. The
  alternative is a per-user store on Windows and macOS and **no equivalent at all on Linux**, where
  the machine-wide anchors directory is the only one there is; browsers there are reached through NSS
  instead, which is T49b and needs no privilege.
- **Whether the machine trusts it is read, never remembered.** `cert.ca_status` asks the store each
  time and the daemon asks at every start. A stored flag would be a claim an OS update, another
  account or a person with `certmgr` could falsify without MixEngine hearing about it, and reading
  costs no privilege on any of the three systems.
- **The helper cannot be aimed at a certificate it did not make.** `TrustCaRemove` carries the CA's
  eight-character key-id and has no field for a fingerprint: one that did would let a compromised
  daemon remove the root that validates Windows Update, through the audited binary and under the
  user's own Allow click. The install's shape check is not a boundary against that attacker — one
  holding the CA key can already sign anything — it exists so `ca-uninstall` can enumerate everything
  an install could ever have created.
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
  SSID/subnet) and never applies to database ports — the API refuses that combination, so no
  client can offer it.
- Generated DB instances get a random 32-char root password stored in the OS keyring, not a blank
  password. `mix service credentials <id>` reveals it on demand.

## Client authentication

**One of these three is built, and the section says which** — because a security document
describing a control that is not there is how a later reader concludes the control exists.

- IPC socket/pipe permissions are the primary control (owner-only). **Built**, and the whole of
  what stands between a client and this daemon today — see
  [daemon-and-ipc.md](daemon-and-ipc.md) for the two gates and the client's own.
- **Not built.** The optional TCP listener requiring `Authorization: Bearer <token>` from
  `run/api.token`, regenerated on every daemon start. There is no TCP listener: T8 left it out on
  purpose as *"a second transport and a second access-control story for a case nobody has yet"*, and
  a token nothing reads guards nothing. If it is ever built, this bullet is its specification.
- **Not built, and not going to be.** Extensions were to get their own scoped token and a declared
  permission set. **T80 refused it** — see
  [ADR 0014](../decisions/0014-an-extension-is-not-an-api-client.md). An extension runs as the
  user's own account, and the access control on this endpoint *is* the account, so a token an
  extension held is one it could put down: it would open its own connection, unauthenticated, and
  reach everything `mix` reaches. Making it a boundary means requiring a token on **every**
  connection, `mix` included — the second access-control story the bullet above already refused for
  a case nobody has. And nothing has the case: no extension in the plan (Mailpit, phpMyAdmin,
  Adminer, MixDB) calls the daemon API at all.

  What T80 shipped instead: `[permissions]` as a **declaration shown before an extension is
  installed** — the shape T78a gave `[scaffold]` — with the two permissions that can hold enforced
  by the manifest format itself. `network` holds because a manifest cannot write an address:
  `{listen}` renders from `permissions.network` and from nothing else, and a host written out
  anywhere in the file is refused at parse. `filesystem = ["own-data"]` holds because every path
  must grow from `{install_dir}` or `{data_dir}`. `permissions.services` is a disclosure, is
  labelled as one on every surface that prints it, and enforces nothing.

## Supply chain

- Every downloaded runtime/package is verified against a SHA-256 pinned in the signed
  `packages.json` index; the index itself is verified with a minisign/Ed25519 public key compiled
  into the binary. A hash mismatch aborts and deletes the download.
- Downloads go over HTTPS with the system roots — **not** our own CA.
- Extension packages are verified the same way; unsigned extensions require an explicit
  `--allow-unsigned` and are marked as such forever, on every surface that lists them.

## What we explicitly do not defend against

Stated so nobody assumes otherwise: MixEngine is a *developer tool on a trusted single-user machine*.
It does not protect against a local attacker who already has the user's account — such an attacker
can edit `mixengine.db` and reach everything MixEngine can.

Specifically: if `mixengine-elevate` is installed somewhere the user can write, malware running as
the user could replace it and gain root the next time the user approves a prompt. We reduce this by
installing it to a root-owned location and keeping it out of the auto-update path, but we do not
claim to eliminate it — it is the same trust model as `sudo` on a personal machine.

**A second account on the machine is a different matter, and is defended against where it costs
little.** "Single-user" describes the machine MixEngine is built for, not a licence to hand a
stranger the API: another *account* is not the user, holds none of the user's data, and every place
one could reach in is closed rather than argued away. Both ends of the local endpoint therefore name
an account and check the one at the other end — including the client, which on Windows can otherwise
be led to a pipe an unprivileged account created under the name it was about to dial
([daemon-and-ipc](daemon-and-ipc.md)). The line above is about an attacker who already *is* the user;
it has never been about anyone else signed in beside them.

Our goal is: no accidental exposure to the network, **no process holding root while idle**, no
unreviewable privilege-escalation path, and no residue left behind at uninstall.
