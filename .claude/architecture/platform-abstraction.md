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
| `Elevation` | run the helper elevated | UAC via `runas`/`ShellExecuteEx` | `AuthorizationExecuteWithPrivileges` → prefer an installed launchd daemon | `pkexec` / polkit policy |
| `ServiceInstaller` | register daemon autostart + helper service | Task Scheduler logon task; helper = Windows Service | LaunchAgent (user) + LaunchDaemon (helper) | systemd user unit + system unit |
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

Only these cross into the helper. The list is closed; adding to it requires an ADR:

```rust
enum PrivilegedOp {
    HostsApply { entries: Vec<HostEntry> },
    ResolverInstall { tld: String, addr: SocketAddr },
    ResolverRemove  { tld: String },
    TrustCaInstall  { der: Vec<u8> },
    TrustCaRemove   { fingerprint: String },
    BindPrivilegedPort { port: u16 },   // returns a passed FD/socket handle
    FirewallAllow { port: u16, label: String },
    FirewallRevoke { label: String },
    PathIntegrationApply { dir: PathBuf },
}
```

See [security-model.md](security-model.md) for how the helper validates these.

## Testing platform code

- Trait-level logic (diffing hosts entries, marker parsing, rule naming) is tested against `mock`.
- Real impls get `#[ignore]`-by-default integration tests run only in CI's per-OS elevated job.
- Anything that edits a system file must have a test proving that unrelated lines survive a
  write/rollback cycle. That regression is the one users will never forgive.
