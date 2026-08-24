# Rust standards

## Toolchain

- Edition 2024, MSRV 1.97.1 pinned in `rust-toolchain.toml` and bumped deliberately — when it moves,
  `rust-version` in the workspace manifest moves with it.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` gate every commit.
- So does rustdoc: `cargo doc --workspace --no-deps --document-private-items`. Intra-doc links are
  resolved by nothing else, and `rustdoc::all` is denied in `[workspace.lints]`, so this fails
  locally exactly as it does in CI. A link inside an OS-specific module is only resolved when
  documenting *that* OS, and `--target` no longer substitutes for being on it: cargo runs the build
  scripts of the dependencies, and `libsqlite3-sys` compiles SQLite for the target it is given. CI
  therefore documents on each of the three runners, and a change to `mixengine-platform`'s Windows
  or macOS half is checked there rather than from your machine.
- Workspace-level `[workspace.dependencies]`; member crates use `dep.workspace = true`. One version
  of `tokio`, `serde`, `sqlx`, `tracing` across the tree.

## Core dependencies (decided, do not swap casually)

| Concern | Crate |
| --- | --- |
| async runtime | `tokio` (multi-thread) |
| HTTP server/client | `hyper` + `hyper-util`, `reqwest` (rustls) for downloads — see below |
| index signatures | `minisign-verify` in the product, `minisign` in the testkit and nowhere else |
| artifact hashing | `sha2` — pinned to the 0.10 `sqlx` already brings, not the newer 0.11 |
| archives | `zip` (deflate only), `tar`, `flate2`; zstd is `ruzstd` in the product and `zstd` in the testkit, for `minisign`'s reason |
| serialisation | `serde`, `serde_json`, `toml` |
| DB | `sqlx` (SQLite, compile-time checked) |
| CLI | `clap` (derive) |
| logging | `tracing` + `tracing-subscriber` |
| errors | `thiserror` in libraries, `anyhow` only in binaries' `main` |
| templates | `minijinja` |
| certs | `rcgen`, `rustls`, `x509-parser` |
| DNS | `hickory-server` + `hickory-proto`. **Not `hickory-resolver`**: the daemon is authoritative for the TLDs it manages and refuses everything else rather than forwarding (T44 design, D1), so there is no stub resolver and no cache in this workspace |
| mDNS | `mdns-sd` |
| process/system info | `sysinfo` |
| keyring | `keyring`, plus a direct edge onto its Linux backend `dbus-secret-service` — the one exception to this table, argued in [ADR 0013](../decisions/0013-reading-the-d-bus-error-name-to-tell-an-absent-store.md): `keyring` cannot say whether a machine has no secret service or has one that refused, and the D-Bus error name underneath it can |
| paths | `directories` |
| Windows APIs | `windows` (official crate), never `winapi` |
| TS bindings | `ts-rs` |

### Outbound TLS trusts the operating system, not a bundled root store

Settled at T20, which brought the first outbound request in this workspace — `hyper` serves the
local IPC socket and answers loopback health checks, and `mixengine-supervisor` refuses an `https://`
check rather than pull a TLS stack in for `127.0.0.1`.

`reqwest`'s default `rustls` feature enables **`rustls-platform-verifier`**, so certificates are
judged by the OS's own verifier: enterprise roots are honoured, CA constraints and OCSP/CRL
revocation apply, and a machine behind a TLS-inspecting corporate proxy works. Keep it. A bundled
root store would refuse that machine with nothing the user could do, and would refuse the internal
certificate a `MIXENGINE_MIRROR_URL` mirror is likely to carry. `rustls-native-certs` is not the
alternative to reach for either — its own maintainers now point at the platform verifier.

Being permissive there is affordable because **TLS is not what decides whether a document is ours**:
the Ed25519 index signature is, end to end, checked after the bytes arrive however they arrived. One
consequence to know before somebody rediscovers it and calls it a hole: from Phase 5 MixEngine
installs its own CA into that same store, so its own CA is trusted by its own downloader. The private
key sits on the user's machine — anybody holding it already owns the machine — and it cannot touch
the signature.

## Error handling

- Every library crate defines its own `Error` enum with `thiserror`. **No `anyhow::Result` in
  library signatures.**
- `mixengine-proto::Error` is the wire error; conversions live at the daemon boundary and always set
  a stable `code` and a useful `hint`.
- `unwrap()`/`expect()` are allowed only where a panic is genuinely impossible and the message says
  why (`expect("template compiled at build time")`). A panic in the daemon kills every managed
  service, so treat it accordingly: the RPC layer catches panics per request and returns `internal`.
- Never swallow an error to keep going. If a degraded path is intended, log at `warn!` with context
  and reflect it in the returned state (`Degraded`, not `Running`).

## Async

- Blocking work (file extraction, hashing, `sqlx` on a large query, any shell-out that can hang) goes
  through `spawn_blocking` or a dedicated task; nothing blocks the runtime.
- Every spawned task has a name (`tokio::task::Builder`) and a shutdown path via a `CancellationToken`
  derived from the daemon's root token. No detached tasks that outlive shutdown.
- Timeouts on everything that touches the network, a child process, or a socket. A missing timeout is
  a review blocker.

## Structure & style

- Modules by capability, not by layer: `sites/`, `runtimes/`, `certs/` — not `models/`, `utils/`.
  `utils` and `helpers` are banned module names.
- Newtypes for identifiers (`SiteId`, `ServiceId`, `RuntimeKind`); no bare `String` IDs crossing a
  function boundary.
- Public items in `core`, `proto`, `platform` carry doc comments explaining *why*, with `# Errors`
  and `# Panics` sections where applicable.
- **A comment earns its place by carrying what the code cannot**: an alternative that was tried and
  rejected, a constraint found by experiment, a hazard in somebody else's crate. One that restates
  the line below it is a line to delete — and one that restates a note in `.claude/` is worse, since
  two tellings of a decision are two places for it to drift (see
  [../roadmap/todo.md](../roadmap/todo.md), "Working on this file", for which telling wins).
- Where the claim is about **another crate's behaviour** — that `sqlx` reports this as that, that
  `tracing-subscriber` appends recorded fields — prefer a test to a sentence. A sentence is checked
  by nobody and goes stale in silence on the day the dependency is upgraded.
- Constructors take injected dependencies (`Arc<dyn Host>`, `Arc<Store>`); no global singletons, no
  `lazy_static` state, no reading env vars deep inside a function — configuration enters at `main`.

## Logging

- `tracing` spans around every RPC (`method`, `request_id`) and every job (`job_id`). Structured
  fields, not formatted strings: `info!(service = %id, port, "starting")`.
- Levels: `error!` = user-visible failure; `warn!` = degraded but continuing; `info!` = lifecycle
  events a user might care about; `debug!` = developer detail; `trace!` = firehose.
- **Never log secrets**: DB passwords, API tokens, private keys. Where a struct might contain one,
  implement `Debug` manually and redact.

## Platform code

All of it in `mixengine-platform` behind traits — see
[../architecture/platform-abstraction.md](../architecture/platform-abstraction.md). `#[cfg(windows)]`
appearing anywhere else fails review. Shell-outs use argument vectors, never interpolated strings.
