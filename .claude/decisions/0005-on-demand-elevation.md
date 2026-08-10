# 0005. On-demand elevation, no persistent privileged helper

**Status**: Accepted
**Date**: 2026-08-10
**Supersedes the helper design in** [0001](0001-rust-core-daemon-gui-split.md) (the three-tier split
otherwise stands)

## Context

Two constraints arrived after the initial architecture was written:

1. **The app auto-updates from GitHub Releases** and prompts the user to install
   ([../features/updates.md](../features/updates.md)).
2. **The app will not be OS code-signed** on any platform (no Apple Developer ID, no Authenticode).

ADR 0001 assumed a persistent `mixengine-helper` installed as a root-owned system service. Combined
with the two constraints above, that produces a component which runs as root, is auto-updated from
the internet, and carries no OS signature the platform can verify. That is a textbook local
privilege-escalation vector: any code running as the user can overwrite the helper and obtain root
without further interaction.

Separately, a persistent root daemon is disproportionate to what MixEngine actually needs root for.
Auditing the requirement produced two categories:

- **One-shot operations** — edit the hosts file, install the root CA, write resolver/NRPT config, add
  a firewall rule. Rare, fast, transactional.
- **Continuously held privilege** — binding ports 80/443/53 on Unix. This is the only thing that
  genuinely cannot be done "per request".

## Decision

**MixEngine runs entirely without elevated privileges. When a one-shot privileged operation is
needed, it asks the OS at that moment, does the work in a short-lived process, and exits.**

### 1. `mixengine-elevate`, a one-shot binary

Replaces the persistent helper. It is spawned through the OS elevation prompt, reads a typed request,
performs it, writes a response, and exits. It never listens, never persists, never installs itself as
a service.

| OS | Elevation mechanism | Notes |
| --- | --- | --- |
| Windows | `ShellExecuteEx` with the `runas` verb → UAC | No API elevates an existing process; a separate binary is mandatory. Request/response passed as files, since stdio cannot cross the integrity boundary. On a standard-user account UAC asks for admin credentials, not just consent. |
| macOS | `do shell script … with administrator privileges` via osascript | Works unsigned; caches credentials briefly. `AuthorizationExecuteWithPrivileges` is deprecated and must not be used. |
| Linux | `pkexec` (polkit) | Requires a running polkit agent. Where none exists, pkexec falls back to a tty prompt a GUI cannot show — we must detect this and print the exact command for the user to run manually. |

The elevated process **validates every request itself**. The daemon runs as the user; if the daemon
is compromised it is the attacker. Validation is therefore duplicated, not delegated:

- Only a list of typed entries is accepted — never a path, command, script, or shell string.
- Domains must match `^[a-z0-9-]+(\.[a-z0-9-]+)*$` and end in a managed TLD.
- Only the region between `# BEGIN MixEngine` / `# END MixEngine` may be touched.
- Writes are atomic (temp file in the same directory → fsync → rename; `ReplaceFile` on Windows to
  preserve ACLs) and guarded by an advisory lock.
- Every invocation appends to a root-owned audit log.

### 2. Privileged ports are designed away, not requested

| Port | Windows | macOS | Linux |
| --- | --- | --- | --- |
| 80 / 443 | bind directly — Windows has no privileged-port concept | pf anchor redirecting 80→8080, 443→8443, written once | `setcap cap_net_bind_service=+ep` on the web server binary, or an nftables redirect |
| 53 (DNS) | bind 53 directly (no admin needed) | bind **5353**; `/etc/resolver/<tld>` supports a `port` directive | bind **5353**; `resolvectl dns <link> 127.0.0.1:5353` / dnsmasq `server=/test/127.0.0.1#5353` |

The DNS case resolves cleanly on all three: the only platform whose resolver mechanism (NRPT) cannot
express a custom port is also the platform that lets an unprivileged process bind port 53.

**Known trap**: `setcap` is attached to the binary and is lost when an update replaces it. The daemon
must detect the missing capability after every update and re-request it. Prefer the redirect approach
where practical.

### 3. Batching and graceful degradation

- Pending privileged operations are queued and flushed in a **single** elevated invocation. Elevating
  inside a loop is a defect.
- A declined prompt is a normal outcome, never an error. The system degrades — hosts-only mode,
  HTTP-only, wildcards disabled — records what is pending, and surfaces a "grant permission" action.

## Consequences

**Easy**:

- Nothing MixEngine ships runs as root between operations, so the unsigned auto-update path has no
  privileged target to hijack.
- No system service to install, keep alive, version-negotiate, or clean up at uninstall — on three
  platforms.
- macOS `SMJobBless` is avoided entirely; it requires signing and matching team IDs, which we do not
  have.
- Realistic prompt count over the whole lifetime is about two: one at first run (CA + resolver +
  port setup, batched), one when enabling LAN sharing, plus one at uninstall.

**Hard / accepted costs**:

- **Site creation must not require a prompt**, which makes the internal DNS server the primary
  mechanism for local domains and demotes the hosts file to a fallback — the reverse of the original
  design ([../features/domains-and-dns.md](../features/domains-and-dns.md)).
- Three elevation mechanisms with meaningfully different failure modes, the Linux polkit-agent gap
  being the worst.
- `mixengine-elevate` sitting in a user-writable directory can still be replaced by malware, which
  then obtains root the next time the user approves a prompt. This is weaker than the persistent-root
  vector (it requires user interaction) and is consistent with the trust boundary already stated in
  [../architecture/security-model.md](../architecture/security-model.md). Mitigation: install it to a
  root-owned location and exclude it from auto-update.
- Some zero-touch polish is lost; a few operations now show an OS dialog.

## Alternatives considered

- **Persistent root helper (the original ADR 0001 design).** Best UX, zero prompts after setup.
  Rejected: unacceptable when combined with unsigned auto-update, and disproportionate to a
  requirement that is almost entirely one-shot.
- **Sign the helper and keep it persistent.** Would close the security hole but costs ~$99/yr (Apple)
  plus Authenticode, which is explicitly out of scope for now. Revisit if the project ever buys
  certificates — the two decisions are linked.
- **Run everything on high ports and skip privileged operations altogether** (`:8080`, no hosts, no
  trust store). Zero prompts, but it discards the features that justify the product: clean URLs and
  working HTTPS.
- **Ask for elevation once and hold it for the session.** Not expressible on Windows or macOS without
  a persistent privileged process, which is what this ADR removes.
