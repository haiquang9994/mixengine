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
    mock/           always compiled; in-memory impl used by tests and `--dry-run`
```

`Host` is a bundle trait exposing each capability; the daemon takes `Arc<dyn Host>` at construction,
so tests inject `mock::Host` and assert on recorded operations.

## The traits

| Trait | Purpose | Windows | macOS | Linux |
| --- | --- | --- | --- | --- |
| `HostsFile` | add/remove/list managed entries | `%SystemRoot%\System32\drivers\etc\hosts` | `/etc/hosts` | `/etc/hosts` |
| `ResolverConfig` | route a TLD to our DNS | NRPT rule (`Add-DnsClientNrptRule`) | `/etc/resolver/<tld>` | `systemd-resolved` per-link domain, else NM/dnsmasq drop-in |
| `TrustStore` | install/remove the root CA | `certutil -addstore ROOT` / CryptoAPI | `security add-trusted-cert -d -k /Library/Keychains/System.keychain` | `/usr/local/share/ca-certificates` + `update-ca-certificates`, plus NSS DBs via `certutil -d sql:~/.pki/nssdb` |
| `Elevation` | run `mixengine-elevate` once, elevated | `ShellExecuteEx` verb `runas` → UAC | `do shell script … with administrator privileges` via osascript | `pkexec` (polkit); detect a missing agent and fall back to printing the command |
| `ServiceInstaller` | register daemon autostart (user-level only) | Task Scheduler logon task | LaunchAgent | systemd **user** unit |
| `PortAccess` | make 80/443 reachable without root | no-op — Windows has no privileged ports | pf anchor redirect 80→8080, 443→8443 | `setcap cap_net_bind_service`, or nftables redirect |
| `ProcessLimits` | cap CPU/memory of a child | Job Object limits | `setpriority` + watchdog | cgroup v2 slice |
| `FirewallRules` | allow LAN access to a port | `netsh advfirewall` | pf / no-op (app firewall prompt) | `ufw`/`firewalld` if present, else advisory |
| `NetworkInfo` | LAN IPs, active interface | `GetAdaptersAddresses` | `getifaddrs` | `getifaddrs` |
| `Keyring` | store service passwords | Credential Manager | Keychain | libsecret |
| `PathIntegration` | put `<root>/bin` on PATH | user `Path` env var | `~/.zprofile`/`~/.bash_profile` + `/etc/paths.d` | shell profile drop-in |

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

## Privileged operations

Only these cross into `mixengine-elevate`. Every one is **one-shot** — there is no operation that
holds privilege. The list is closed; adding to it requires an ADR:

```rust
enum PrivilegedOp {
    HostsApply     { entries: Vec<HostEntry> },
    ResolverInstall{ tld: String, addr: SocketAddr },   // addr may carry a non-53 port
    ResolverRemove { tld: String },
    TrustCaInstall { der: Vec<u8> },
    TrustCaRemove  { fingerprint: String },
    PortAccessGrant{ binary: PathBuf, ports: Vec<u16> },// setcap / pf anchor / no-op
    PortAccessRevoke,
    FirewallAllow  { port: u16, label: String },
    FirewallRevoke { label: String },
    PathIntegrationApply { dir: PathBuf },
}
```

Requests are submitted as a **batch** (`Vec<PrivilegedOp>`) so one prompt covers everything pending.
Execution is all-or-nothing per operation and reports per-operation results; a partially applied batch
is reported, never silently ignored.

`setcap` is attached to the binary and is **lost whenever an update replaces it**. The daemon must
re-probe port access after every update and re-request `PortAccessGrant` if needed — prefer the
redirect approach where the platform supports it.

See [security-model.md](security-model.md) for how the elevated process validates these, and
[../decisions/0005-on-demand-elevation.md](../decisions/0005-on-demand-elevation.md) for why it works
this way.

## Testing platform code

- Trait-level logic (diffing hosts entries, marker parsing, rule naming) is tested against `mock`.
- Real impls get `#[ignore]`-by-default integration tests run only in CI's per-OS elevated job.
- Anything that edits a system file must have a test proving that unrelated lines survive a
  write/rollback cycle. That regression is the one users will never forgive.
