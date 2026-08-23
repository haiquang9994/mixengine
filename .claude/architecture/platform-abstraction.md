# Platform abstraction

Everything the OS does differently lives in `mixengine-platform`. Core, daemon, supervisor and
clients contain **zero** `#[cfg(target_os = …)]`.

## Shape

```
mixengine-platform/
  src/
    lib.rs          re-exports traits + `pub fn host() -> Box<dyn Host>`
    traits/         one file per trait
    windows/        impls behind cfg(windows)
    macos/          impls behind cfg(target_os = "macos")
    linux/          impls behind cfg(target_os = "linux")
    unix/           what macos/ and linux/ do identically; each names what it takes
    mock/           always compiled; in-memory impl used by tests and `--dry-run`
```

`unix/` is not a fourth platform. It exists so a capability that is genuinely POSIX — file modes,
signals, process groups — is written once instead of copied into two directories and left to drift.
Anything macOS and Linux do differently stays in their own directory.

`secrets.rs` sits at the top level for the same reason one step further out: `Keyring` has **one**
implementation, not three, because the `keyring` crate is already the abstraction — Credential
Manager, Keychain and D-Bus secret service behind one API. Which backend each OS gets is chosen by a
feature in `Cargo.toml` (`windows-native`, `apple-native`, `sync-secret-service` + `crypto-rust` +
`vendored`) rather than by code, and the crate's default — no feature, a backend that stores nothing
and reads back nothing — is never selected, so a missing choice is a build failure and not a keyring
that silently forgets. The synchronous secret-service backend is deliberate: the async one blocks on
an executor of its own inside a synchronous call, which panics on a runtime worker thread, and every
caller is a daemon that has one. `vendored` compiles libdbus in, so building a release does not
require `libdbus-1-dev` on the machine doing it.

`Host` is a bundle trait exposing each capability; the daemon takes `Arc<dyn Host>` at construction,
so tests inject `mock::Host` and assert on recorded operations.

**Four modules are deliberately not behind `Host`:** `ipc`, `lock`, `signal` and `process`. A
capability is a question about the machine, asked of an injected object so a test can answer it from
memory. These four are not questions — they are a concrete listener and byte stream, a held file
handle, an installed signal handler and a started process — and a mock of any of them would prove
nothing about the OS mechanism that is the entire content of the task: socket permissions and a pipe
DACL, `flock` against a share mode, `SIGTERM` against five console control events, `setsid` against
`DETACHED_PROCESS`. Each is a plain module with a `sys::…` behind it, exercised against the real OS
in `tests/`, touching only a `TempDir` and so needing no `#[ignore]`.

## The traits

| Trait | Purpose | Windows | macOS | Linux |
| --- | --- | --- | --- | --- |
| `HomeDirs` | where the root goes when the user picks nothing | `%LOCALAPPDATA%\MixEngine` | `~/Library/Application Support/MixEngine` | `$XDG_DATA_HOME/mixengine` |
| `DirectoryAccess` | keep other local accounts out of `certs/`, `data/`, `run/` | `icacls /inheritance:r` + a DACL naming the user's SID, `SYSTEM` and `Administrators` | `chmod 0700` | `chmod 0700` |
| `HostsFile` | read the managed block (the write is `PrivilegedOp::HostsApply`) | `%SystemRoot%\System32\drivers\etc\hosts` | `/etc/hosts` | `/etc/hosts` |
| `ResolverConfig` | route a TLD to our DNS (read; the write is `PrivilegedOp::ResolverApply`) | one NRPT rule, written as registry values under a fixed GUID — **not** `Add-DnsClientNrptRule`, which is a scripting host started by a process holding an administrative token | `/etc/resolver/<tld>`, one marked file per TLD; a file MixEngine did not write is refused, never replaced | a `systemd-networkd` **dummy link of our own** carrying `10.53.53.53/32` — measured, and *not* the link-local address an earlier draft named: that was true of a link brought up with `ip addr add` and was never measured for a link declared in these files; a `resolved.conf.d` drop-in redirects the whole machine, `resolvectl dns lo` is refused by name, a real link has its servers replaced, and a link with no address never gets a DNS scope |
| `TrustStore` | install/remove the root CA | `certutil -addstore ROOT` / CryptoAPI | `security add-trusted-cert -d -k /Library/Keychains/System.keychain` | `/usr/local/share/ca-certificates` + `update-ca-certificates`, plus NSS DBs via `certutil -d sql:~/.pki/nssdb` |
| `Elevation` | run `mixengine-elevate` once, elevated | `ShellExecuteEx` verb `runas` → UAC | `do shell script … with administrator privileges` via osascript | `pkexec --disable-internal-agent`, after an environment check for an agent; no `.policy` file is shipped, and a machine with no agent is told the command to run by hand |
| `ServiceInstaller` | register daemon autostart (user-level only) | Task Scheduler logon task | LaunchAgent | systemd **user** unit |
| `PortAccess` | make 80/443 reachable without root | no-op — Windows has no privileged ports | pf anchor redirect 80→8080, 443→8443, plus a LaunchDaemon that enables pf at boot ([ADR 0012](../decisions/0012-a-boot-time-job-enables-the-packet-filter-on-macos.md)) | `cap_net_bind_service` on the front-end binary, written as the `security.capability` xattr rather than through `libcap` |
| `PortOwner` | say who is already listening on a port, so a failed start can name them (T38) | `GetExtendedTcpTable` + `QueryFullProcessImageNameW` | `lsof -t` + `ps -o comm=` | `/proc/net/tcp[6]` + a walk of `/proc/<pid>/fd` |
| `ReservedPorts` | say which ranges the **operating system** has taken out of circulation (T47a) | `netsh int ipv4 show excludedportrange`, parsed — the registry's `ReservedPorts` holds what an administrator asked for, while `netsh` holds what the system actually took, including what Hyper-V and `winnat` claim at boot | `Unsupported`: macOS reserves nothing | `Unsupported`: nothing outside `ip_local_reserved_ports`, empty on every ordinary machine |
| `ProcessLimits` | cap CPU/memory of a child | Job Object limits | `setpriority` + watchdog | cgroup v2 slice |
| `FirewallRules` | allow LAN access to a port | `netsh advfirewall` | pf / no-op (app firewall prompt) | `ufw`/`firewalld` if present, else advisory |
| `NetworkInfo` | LAN IPs, active interface | `GetAdaptersAddresses` | `getifaddrs` | `getifaddrs` |
| `Keyring` | store service passwords | Credential Manager | login Keychain | D-Bus secret service (gnome-keyring, kwallet) — absent on a headless box, where the answer is `UnsupportedPlatform` |
| `PathIntegration` | put `<root>/bin` on PATH | `HKCU\Environment\Path`, prepended, type preserved | marked block in `~/.zprofile`, `~/.bash_profile`, `~/.profile` | the same, `~/.profile` first |

**Three of the traits above are about ports, and they answer three different questions.** `PortOwner`
says who got there first. `PortAccess` says whether an unprivileged program may bind a *low* port at
all. `ReservedPorts` says whether the operating system has taken the range out of circulation
entirely — and that last one earns a trait of its own because a bind into a reserved range fails with
an **access error**, so it reads as a permission problem: a person who hits it goes looking at
elevation, UAC and the firewall, and none of them is the answer.

## Rules

1. **Every mutation is reversible and tagged.** Managed hosts entries are wrapped in
   `# BEGIN MixEngine` / `# END MixEngine` markers; we never touch lines outside the block. Same idea
   for resolver files and firewall rule names (`MixEngine — <purpose>`).
2. **Read-modify-write is atomic.** Write to a temp file in the same directory, `fsync`, then rename.
   On Windows use `ReplaceFile` to preserve ACLs. Take an advisory lock so two MixEngine processes
   never interleave.
3. **Detect, then act.** Each impl exposes `probe()` returning what is actually available
   (e.g. is `systemd-resolved` in use? is NSS present?) so we degrade instead of failing.
4. **`Unsupported` is a valid answer.** Return `Error::UnsupportedPlatform { capability, reason }`
   with a hint describing the manual workaround. Never `unimplemented!()`.
5. **Shell-outs are the exception and are argument-vector calls**, never string-interpolated command
   lines. If a Windows API exists (`windows` crate), use it instead of spawning `powershell`.
   `DirectoryAccess` is the standing exception: setting a DACL through `SetNamedSecurityInfoW` means
   hand-computing ACL sizes behind raw pointers, where a mistake yields a *wrong* ACL rather than a
   crash — and the crates that wrap it safely have been frozen on the unmaintained `winapi 0.3`
   since 2021. `icacls` is called with an argument vector and names well-known accounts by SID, so a
   localised Windows cannot change what a grant means. The price is paid on the reading side, not
   the writing one: `icacls` prints localised names, so the capability can verify that inheritance
   was severed but not who holds the remaining ACEs. T47 owns the decision to revisit that.

## Privileged operations

Only these cross into `mixengine-elevate`. Every one is **one-shot** — there is no operation that
holds privilege. The list is closed **against operations with effects**; adding one of those requires
an ADR:

```rust
enum PrivilegedOp {
    Probe,                                             // reports; changes nothing
    HostsApply     { entries: Vec<HostEntry> },
    ResolverApply  { plan: ResolverPlan },         // which managed TLDs, and on which port
    ResolverRevoke { target: ResolverTarget },    // and the reverse of each
    TrustCaInstall { der: Vec<u8> },
    TrustCaRemove  { fingerprint: String },
    PortAccessGrant{ plan: PortAccessPlan },      // setcap / pf anchor + boot job / refused
    PortAccessRevoke{ target: PortAccessTarget }, // and the reverse of each
    FirewallAllow  { port: u16, label: String },
    FirewallRevoke { label: String },
}
```

**Resolver wiring was on this list in a wider shape than it landed in** (T45). It read
`ResolverInstall { tld: String, addr: SocketAddr }` — one TLD per operation, and an address the
request got to choose. Both are gone: the operation is whole-state, and `127.0.0.1` is compiled into
the helper along with the Linux link's name and address and the Windows registry GUID. **That needed
no ADR**, because the rule above exists to stop a new capability being granted quietly and this
grants strictly less: an operation that could have pointed the machine's name resolution anywhere may
now point it only at loopback, and one that carried a name may now carry only a member of a
compiled-in table. Narrowing an entry is the same direction as removing one.

**`PathIntegrationApply` used to be on this list and is not** (T26). Every one of the three systems
keeps the current user's PATH somewhere that user can already write: `HKEY_CURRENT_USER\Environment`
on Windows, a file in `$HOME` on both others. `/etc/paths.d` — the macOS mechanism the table above
used to name — is the one place that would have needed root, and it is root's precisely because it
is machine-wide, which is the opposite of what a per-user development tool wants. So `path.install`
is an ordinary API method, and nobody is asked for a password to add a line to their own
`.zprofile`. Removing an entry from this list needs no ADR; **adding one does**.

**`Probe` is the one member that changes nothing**, and the only one whose `requires_elevation()` is
`false` (T40). It reports the helper's version, whether the process is in fact elevated, which
operations the build knows and where its audit log is — which is how a daemon finds out what the
*installed* helper can do without spending a prompt to discover it by failure. That matters because
the helper is excluded from auto-update: an old helper meeting a new daemon is a certainty rather
than a risk. It was added without an ADR deliberately: the rule exists to stop a new capability being
granted quietly, and a non-mutating self-report grants none. Removing an entry needs no ADR, for the
symmetric reason, and T26 already did it.

**Elevation is a property of the operation, not a gate on the process.** The obvious frame refuses to
do anything at all when the helper is not running elevated; `Probe` is what shows that to be wrong,
since the operation whose job includes reporting whether the token is elevated could then never
report `false`. The helper applies `requires_elevation()` at one place, and an operation that needs a
privilege the process does not hold is refused **at its own index**.

Requests are submitted as a **batch** (`Vec<PrivilegedOp>`) so one prompt covers everything pending.
Execution is all-or-nothing per operation and reports per-operation results; a partially applied batch
is reported, never silently ignored.

`setcap` is attached to the binary and is **lost whenever an update replaces it** — by any write to
the file, measured, not only by an update. So the daemon **probes on every start** and enqueues a
`PortAccessGrant` when the probe says the grant is gone: that catches a loss an update did not cause,
and needs no hook in the updater. It is affordable because reading the attribute back costs one
`getxattr` and no privilege at all. **This is what closes T88b**, which asked for a re-probe after
every update alone.

See [security-model.md](security-model.md) for how the elevated process validates these, and
[../decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md) for why it works
this way.

## Testing platform code

- Trait-level logic (diffing hosts entries, marker parsing, rule naming) is tested against `mock`.
- Real impls get `#[ignore]`-by-default integration tests run only in CI's per-OS elevated job.
- Anything that edits a system file must have a test proving that unrelated lines survive a
  write/rollback cycle. That regression is the one users will never forgive.
