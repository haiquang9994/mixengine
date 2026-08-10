# MixEngine build plan

Tasks are ordered. Work top to bottom — each phase depends on the ones above it. Tick items as they
land; when new work appears, insert it **where it belongs in the order**, not at the end.

Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** = has a platform-layer component and
needs verification on Windows + macOS + Linux.

---

## Phase 0 — Foundations
*Goal: an empty but real system — daemon starts, CLI talks to it, state persists.*

- [x] **T1** Cargo workspace scaffold: the seven crates from [CLAUDE.md](../../CLAUDE.md) with their
      dependency direction enforced, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`.
      Direction is enforced by `crates/mixengine-proto/tests/workspace_layering.rs`; `cargo deny` is
      configured but only runs once T2 installs it in CI.
- [x] **T2** CI skeleton: `lint` + `test` jobs on windows/macos/ubuntu, network egress blocked for tests.
      `.github/workflows/ci.yml`; egress is blocked for real on Linux via
      `.github/scripts/test-no-network.sh` (private network namespace) and by `--offline` cargo
      everywhere. ESLint/`tsc` steps are written but skip themselves until T55 creates `apps/desktop`.
- [x] **T2a** `cargo doc --workspace --no-deps --document-private-items` with
      `RUSTDOCFLAGS=-D warnings` in the `lint` job. Found in T3b: a broken intra-doc link
      (`crate::macos::access`, which does not exist — every OS directory is mapped onto `sys` by
      `#[path]`) sat in a committed file and neither `clippy` nor `cargo test` said a word, because
      neither runs rustdoc. Doc tests are run today; the docs themselves are never built.
      Runs **once per OS target**, not once: `#[path]` compiles exactly one of `windows/`, `macos/`,
      `linux/`, so a host-only run leaves two thirds of `mixengine-platform` undocumented — checked
      by putting the original broken link back into `macos/access.rs`, where the Linux target still
      passed and only the macOS one failed. Architecture is irrelevant to the docs, so three targets
      cover the six in `deny.toml`, and rustdoc never links, so the two foreign ones cost a
      `rustup target add` and no cross-linker. The workspace documents clean on all three today.
      `rustdoc::all` is denied in `[workspace.lints]` as well, so a plain `cargo doc` fails on a
      developer machine and the rule is not something only CI knows; `RUSTDOCFLAGS` stays for the
      rustc warnings rustdoc's compile pass raises, which a `[lints]` table cannot express.
      Not a docs-coverage gate: `missing_docs` exempts private items even under
      `--document-private-items`, verified by deleting a private doc comment and watching it pass.
      Moved out of `lint` and into the `test` matrix in T6, one run per runner on its own OS: the
      "rustdoc never links, so a foreign target costs a `rustup target add`" premise stopped holding
      the moment a dependency compiled C. rustdoc still does not link, but cargo runs
      `libsqlite3-sys`'s build script anyway, and `cargo doc --target <other OS>` now fails in
      `cc-rs` with "failed to find tool x86_64-linux-gnu-gcc" — reproduced before the step was
      moved. The coverage is identical; only the runner it happens on changed.
- [x] **T3** Paths & config: `MIXENGINE_HOME` resolution per OS, directory bootstrap, `config.toml`
      loading with defaults. **(P)**
      `mixengine-platform` gained its trait shape (`traits/`, `windows/`, `macos/`, `linux/`,
      `mock/`, `host()`) with `HomeDirs` as its first capability; `core::paths::Paths` owns the
      layout and `core::config` the file. `core::open_home` is the one place the four startup steps
      are ordered. `config.toml` holds `[log]`, `[daemon]` and `[paths]` only — further sections
      arrive with the task that reads them; unknown keys are refused rather than ignored.
- [x] **T3a** Owner-only permissions on the home directory: `0700` on the root, `certs/`, `data/`
      and `run/` on Unix and the matching ACL on Windows, applied during bootstrap and re-checked by
      `mix doctor`. Needs a platform capability — `core::paths::create_dir` must stay OS-agnostic.
      Found reviewing T3: directories are created with the process umask (`0755` on most Unix
      machines), which would leave the CA private key (T48) and database data readable by every
      other local user. **(P)**
      `DirectoryAccess` is the capability; `Paths::bootstrap` restricts each private directory the
      moment it creates it, and refuses to start if the OS will not. Windows was the same bug, not
      a smaller one: `C:\` grants `BUILTIN\Users` read with `(OI)(CI)`, so only a home under
      `%LOCALAPPDATA%` was ever safe and a `[paths]` override onto another disk never was.
      `is_restricted_to_owner` is there for T47 and is narrow on Windows — it verifies that nothing
      is inherited and that exactly three ACEs remain, but not *who* they name, because `icacls`
      prints localised account names and never a SID.
- [x] **T3b** Clear inherited ACLs on macOS, the way `/reset` does on Windows (T3a). macOS ACLs are
      NFSv4-style and sit beside the mode, so `chmod 0700` leaves an ACE granting another user in
      place and working; Linux is unaffected, its POSIX ACL being masked by the group class the
      mode sets to zero. **(P)**
      Reproduced on macOS 15 before writing anything: a directory carrying
      `group:everyone allow list,search` reports `drwx------+` after `chmod 0700` and stays
      listable, so the task was real. `macos/access.rs` now wraps the shared `unix/access.rs` —
      mode first, then the ACL — and both halves of the Windows test are ported
      (`macos_removes_an_acl_somebody_else_left_behind`,
      `macos_reports_an_acl_as_unrestricted_despite_a_correct_mode`), each verified to fail against
      the old mode-only implementation. macOS turns out to inherit ACLs as well — from the parent
      directory rather than from the volume — so `windows_severs_an_inherited_ace` has a
      real counterpart too (`macos_severs_an_inherited_ace`): an ACE carrying `directory_inherit` on
      any parent of `MIXENGINE_HOME` lands on every directory created below it, and `file_inherit`
      reaches the files. Unlike Windows this calls `sys/acl.h` rather than the CLI:
      nothing is built, so there is no ACL-sizing hazard, and `ls -le` has no promised format to
      parse. `acl_delete_file_np` is *not* the function for it — Darwin answers `ENOTSUP` for
      `ACL_TYPE_EXTENDED`; setting an empty `acl_init(0)` is what `chmod -N` does and is idempotent.
      The `acl_*` family has no binding in `libc` on Apple targets, so the four entry points are
      declared here under `#[expect(unsafe_code)]`.
- [x] **T4** Logging: `tracing` setup, file + stderr sinks, `MIXENGINE_LOG_FORMAT=json`, rotation of
      `daemon.log`.
      Both sinks always and at one level: the daemon normally runs detached, so the question asked
      hours later is answered by `logs/daemon.log`, and only stderr is ever coloured — escape codes
      in the file would make "copy diagnostics" (T66) produce something no bug report can use. The
      format comes from `log.format`, from `--log-format`, or from `MIXENGINE_LOG_FORMAT`, which
      exists because a collector wraps a command it did not write and cannot add a flag to it; an
      unrecognised value fails the start rather than quietly emitting text nobody is collecting.
      Rotation is size-based (10 MB × 5, matching a service log) and written here rather than taken
      from `tracing-appender`, which only rotates on a clock.
      The one thing that had to be found rather than designed: the file handle must be dropped
      *before* the rename. Rust opens files without `FILE_SHARE_DELETE`, so Windows refuses to
      rename one this very process holds open — with the two lines the other way round three of the
      rotation tests fail on Windows and none on Linux or macOS, checked by doing exactly that. They
      run against a 32-byte limit, not a 10 MB one: crossing the boundary is the whole rule.
      A rotation that fails anyway does not cost the line that triggered it — `tracing` discards a
      writer error, so the file grows past its limit and says so *in itself*, once per run of
      failures rather than once per line. That note is the one line `tracing` cannot write: an
      event would go straight back into the writer whose mutex the failing write still holds, so
      it is composed by hand — which means it also has to obey `log.format`, or a collector reading
      one JSON object per line would meet a sentence of prose at exactly the moment something is
      already wrong. It is handed to the sink rather than written from the file, because the file
      it is about is the one that is failing and under a collector nobody is reading it; stderr is.
      The sink is a `MakeWriter` of our own for the same reason `unwrap` is not used near a logger:
      `tracing-subscriber`'s `Mutex` implementation panics on a poisoned lock, so one panic
      anywhere near logging would silence every line after it, including the ones about the panic.
      `Paths::daemon_log_file` is the only path built on another one (`logs/`) instead of on the
      root, so a `[paths] logs` override onto a second disk takes the daemon's own log with it.
- [x] **T5** Error model: `mixengine-proto::Error` with stable codes + hints; per-crate `thiserror`
      enums and conversions at the daemon boundary.
      `Error` is `{ code, message, hint? }` and `ErrorCode` is the closed set from
      [daemon-and-ipc.md](../architecture/daemon-and-ipc.md) — closed where the library enums are
      `#[non_exhaustive]`, because a new *code* should stop every `match` in the CLI, the GUI and
      the mapping from compiling until somebody has decided what it means. The wire strings are
      spelled out in `as_str` rather than derived by `serde(rename_all)`: they are published, and a
      rename refactor should have to say so out loud. Four of them (`already_exists`, `conflict`,
      `port_in_use`, `privileged_required`) have no producer yet and are vocabulary for Phase 3-4.
      A code this build has never heard of deserialises to `internal` instead of failing: the
      situation is a client older than its daemon, and refusing the payload would replace the
      daemon's actual diagnosis — which is in `message`, and still makes sense — with "invalid
      response" at the one moment something is already wrong.
      The conversion is a `ToWire` trait in the daemon rather than `From`, which the orphan rule
      forbids with both types foreign. It does three things the libraries cannot: flattens the
      `source()` chain into the one string a client is handed, chooses the code, and writes the
      hint where the daemon knows something the library did not — `create` returning `EACCES` is
      all `core` knows; that MixEngine never elevates its way out of it and that `[paths]` exists
      is knowledge that lives here. Where the library message already names the way out
      (`EmptyHome`, `NoHomeDirectory`, `UnsupportedPlatform`, `Command`) the hint stays `None`,
      since the GUI renders both and would otherwise print the same sentence twice.
      Already load-bearing rather than waiting for T8: `main` maps its own startup failure through
      it, so the mapping is exercised and the hint reaches the person reading stderr. Two things
      had to be found rather than designed — `#[error(transparent)]` keeps the inner error as the
      source *and* borrows its message, so a naive walk prints it twice (delegated instead, with a
      guard for the next one), and `toml::de::Error` ends its multi-line complaint with a newline,
      which put a blank line between message and hint until every piece of the chain was trimmed.
- [x] **T6** SQLite store: `sqlx` setup, WAL, migration runner, the schema from
      [data-model.md](../architecture/data-model.md), pre-migration backup.
      `core::store::Store` owns the pool, and the migrations sit next to it in
      `crates/mixengine-core/migrations/` rather than in the daemon as
      [data-model.md](../architecture/data-model.md) originally said — `sqlx::migrate!` embeds the
      directory of the crate it is written in, and the type the domain modules are handed
      (`Arc<Store>`) is a `core` type. The daemon is still the only process that opens the file; the
      spec has been corrected rather than worked around. Connection settings are spelled out
      instead of inherited (WAL, `synchronous=NORMAL`, `foreign_keys` — which is per *connection*,
      so the test holds the whole pool open and asks each one — a five-second busy timeout, four
      connections) and every table is `STRICT`.
      The backup is `VACUUM INTO`, not `std::fs::copy`, and that is the one thing here that had to
      be found rather than designed: under WAL the newest commits are in the `-wal` sidecar until a
      checkpoint moves them, so a file copy silently omits exactly the work the user did last. It
      runs whenever a database that already has a migration applied is about to get another —
      "which migrations destroy data" is not knowable from the SQL, and a fresh database has nothing
      to copy. An existing same-version backup is kept rather than replaced, because in that pair it
      is the older file that predates the half-finished attempt.
      That last rule only holds because the copy goes to a `.partial` and is renamed into place —
      a review finding. "Is there already a backup?" is answered by looking for a file, so a copy
      killed part way through would have answered it wrongly and the next upgrade would have stepped
      over a truncated database, leaving the user a safety net that was not one. After the rename a
      file at that path can only have come from a copy that finished, which is a cheaper guarantee
      than checking the file afterwards and a stronger one than trusting it.
      Three failures, not one, and the third is the one worth spelling out: sqlx reports the
      bookkeeping around a migration (`ensure_migrations_table`, reading the applied versions) as
      `MigrateError::Execute`, which is *not* a migration failing — it is the file being unusable,
      for the ordinary reasons a read-only volume, a full disk or a `mixengine.db` that is not a
      database are. Since that path runs on every daemon start, folding it into "our SQL is wrong"
      would have greeted a home on a read-only disk with "report a bug". It maps to `Database`
      (`io`) instead; only `ExecuteMigration` is `Migration` (`internal`, and the hint can promise
      the database is untouched because SQLite has transactional DDL), and a version that does not
      line up is `IncompatibleDatabase` (`precondition_failed`, hint names the backup). `Dirty` is
      unreachable here for that same transactional-DDL reason, and is mapped rather than left to
      the catch-all so that if it ever does arrive it points at the backup. Also found: with a single embedded migration every database is either empty
      or current, so the backup path has no way to happen in a test — `open_with` takes a `Migrator`
      and the unit tests build two-step sets on disk at run time. And `STRICT` is narrower than it
      sounds: it converts text that *is* an integer and refuses only text that is not, which the
      schema test now states in both directions.
      `build.rs` exists for one line. `sqlx::migrate!` reads the directory while the macro expands
      and cannot tell cargo what it read, so without `rerun-if-changed` an edited migration leaves
      the crate looking unchanged and the tests pass against the previous schema.
      A site's domains ended up in `site_domains` alone, with no `primary_domain` column on `sites`,
      and that was a review finding rather than the first draft: two unique indexes on two tables
      cannot constrain each other, so the split version left `blog.test` free to be site A's primary
      *and* site B's alias at the same time — the exact collision the uniqueness exists to stop, and
      one the original test could not have caught because it only ever wrote to `sites`. One table
      means one index decides ownership and it cannot disagree with itself. "At least one primary
      per site" stays outside the schema: SQLite has no deferred constraint, so it is an invariant
      the site module upholds inside the transaction that creates a site.
      Not here yet: `query!`'s compile-time checking needs committed `.sqlx` offline data and a CI
      check that it is current. It arrives with the first query, in T14 — there is not one in this
      task.
- [x] **T7** IPC transport: Unix socket + Windows named pipe with owner-only permissions and peer
      credential checks. **(P)**
      `mixengine_platform::ipc` — the one thing in that crate deliberately *not* behind `Host`. A
      capability is a question answered by an injected object so a test can answer it from memory;
      this is a concrete listener and a concrete byte stream, and a mock one would prove nothing
      about socket permissions or a pipe DACL, which are the entire content of the task. It hands
      back `AsyncRead + AsyncWrite` and speaks no protocol: HTTP and JSON-RPC are T8's.
      The Windows endpoint is `\\.\pipe\mixengine.<sid>.<fingerprint of run/>`, a correction to
      [daemon-and-ipc.md](../architecture/daemon-and-ipc.md), which said the SID alone. The pipe
      namespace is flat and machine-wide, so the SID alone is one endpoint per *account*, not per
      home: a daemon started with `MIXENGINE_HOME` pointing at a sandbox would collide with the real
      install, and the tests in this task would collide with each other under `cargo test`'s
      parallelism. Unix gets the property for free, the socket being a file inside the home. The
      fingerprint is FNV-1a written out in eight lines rather than taken from a crate — it is a
      name, not a defence; nothing is secret and nothing is authenticated by it. Case-folded first,
      because `C:\dev` and `c:\dev` are one directory and must be one daemon.
      `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance and never on the others, which is the
      difference between claiming a name and silently adding an instance to somebody else's pipe
      and serving whoever they attract.
      **A leftover endpoint is cleaned up and a live one is never touched**, and the two are told
      apart the same way on both systems: by dialling. A socket file outlives the daemon that bound
      it, and Windows reports a taken name as `ERROR_ACCESS_DENIED`, indistinguishable from a real
      permission problem — so the probe answers both. Something answers, and the start fails with
      `EndpointInUse`; nothing does, and the corpse is unlinked. The Unix probe fails *closed*: only
      `ECONNREFUSED` or a missing file prove the socket is dead, because the cost of being wrong the
      other way is unlinking a running daemon's socket. That is not the single-instance guarantee —
      see T9, whose note this sharpens.
      `Drop` unlinks the socket only if its device and inode still match the one that was created,
      which is what stops a shutting-down daemon from deleting a *different* daemon's socket that
      took the same name. The mode is `0600`, set after `bind` because there is no `bind` that takes
      one; the window in which it is `0755` is closed by `run/` already being `0700` from T3a.
      The `sun_path` limit is read out of `libc::sockaddr_un` rather than written down: it is 108 on
      Linux and 104 on macOS, and `unix/` is the one directory that must not branch on which of them
      it is compiled for. Both numbers were confirmed with a temporary `const _: () = assert!(…)`
      cross-compiled at each target rather than trusted. It is checked when the address is built,
      not at `bind`, because `bind` answers a path one byte too long with `EINVAL` — "Invalid
      argument", naming neither the argument nor the limit — and the only thing the user can act on
      is where the home is.
      The peer check is tokio's `peer_cred` on both Unix systems, which is one function over two
      different system calls, so neither OS directory has anything to say about it. Windows
      impersonates the client instead of calling `GetNamedPipeClientProcessId`: a pid has to be
      turned back into a process, by which time the client may have exited and something else may
      hold that number, while the adopted token cannot be about anybody else. Impersonation is a
      property of the *thread* and tokio's workers are shared, so nothing is awaited between
      `ImpersonateNamedPipeClient` and `RevertToSelf` — a future that yielded while impersonating
      would hand a stranger's identity to whatever ran next. A failing `RevertToSelf` aborts the
      process, which is the only proportionate answer and was arrived at by discarding the other
      two: the thread is still carrying the client's identity, and both an `Err` and a panic unwind
      back into the runtime on that same thread, so either one returns a worker to the pool that
      runs the next task — for any client — as whoever just connected. Every managed service is a
      child that outlives the daemon and is picked back up on the next start; a daemon quietly
      acting as somebody else cannot be undone after the fact.
      The accept path has the same shape of hazard one level up. The replacement pipe instance is
      created before the connected one is identified, and if that creation fails the connected
      instance must not be left in the listener: `connect` on it returns `ERROR_PIPE_CONNECTED`,
      which mio reports as success, so every later accept would return instantly and never wait for
      anybody. It is disconnected before the error goes out, which returns it to the state the next
      accept expects.
      Two things had to be found rather than designed. `ImpersonateNamedPipeClient` works
      immediately after `connect`, with nothing yet read — checked, because the alternative would
      have forced the peer check to happen after the first request and therefore inside T8. It works
      because tokio's `ClientOptions` defaults to `SECURITY_IDENTIFICATION | SECURITY_SQOS_PRESENT`,
      which is exactly enough for the server to learn who the client is and nothing more; a client
      that connected anonymously is refused rather than trusted, which is the right way round. And
      the check was proved to be doing something by inverting the comparison and watching two tests
      fail with a real `S-1-5-21-…-1001` — a peer check that silently passes everything looks
      identical to one that works.
      A rejected peer is `Accepted::Untrusted(Peer)` and not an `Err`. The distinction is the whole
      reason the type exists: a stranger knocking is something the accept loop logs and carries on
      from, while an `Err` means the listener itself is in trouble, and collapsing them would make
      the daemon treat "somebody else tried" and "this daemon can no longer accept anything" the
      same way. The daemon's loop paces its retry after an `Err` for the same reason — a per-request
      failure must not end it, and a listener-level one must not spin it at the speed of the CPU.
      Three variants on `mixengine_platform::Error`: `Os` for an OS call with no path to name
      (`io`, not `internal` — the honest reading of a locked-down machine is not "report a bug"),
      `Address` (`invalid_argument`, message already carries the way out) and `EndpointInUse`
      (`conflict`, hint names the daemon that *is* running). `windows-sys` rather than the
      `windows` crate that [rust.md](../standards/rust.md) names: it is already in this tree through
      tokio, mio and socket2, so the security work costs no new package — only three new edges in
      `Cargo.lock` — and no second copy of the same bindings. The security descriptor comes from one
      SDDL string through `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which is the same
      "never hand-compute an ACL" rule `windows/access.rs` argues at length, obeyed with one call
      instead of a shell-out.
      Not covered here, and it needs a second account rather than a mock: a peer check *refusing*
      somebody. What the tests do cover is that it runs on every accept and that this account passes
      it. Also not covered, and noted at T10 because it belongs to the client: on Windows a client
      does not verify the *server*.
- [ ] **T8** JSON-RPC server over the transport: `daemon.status`, `daemon.version`, `/health`,
      `/events` SSE, panic-to-`internal` catch, request spans.
- [ ] **T9** Daemon lifecycle: single-instance lock, `--foreground`/`--detach`, graceful shutdown with
      cancellation token.
      The lock has to be taken *before* `Store::open` in `main`, not after: `sqlx-sqlite` implements
      `Migrate::lock`/`unlock` as no-ops (SQLite has no advisory lock to use), so two daemons
      starting together can both read the schema as behind and both migrate. Nothing starts a second
      daemon today, which is why T6 left it — but a single-instance lock acquired after the database
      is open guards nothing.
      T7 narrowed what is left for the lock to do without replacing it. A second daemon started
      after the first is already refused, by the endpoint itself; what remains is two daemons
      starting at the *same instant*, where both can find the endpoint dead, and on Unix the
      second's `bind` then replaces the first's socket file while the first is still listening on
      it. The `Drop` there keeps that from compounding — a listener unlinks the socket only if its
      device and inode still match the one it created — but the first daemon is left serving an
      endpoint no client can reach, and only the lock prevents that.
- [ ] **T10** CLI skeleton: `clap` tree, transport client, daemon autostart-on-connect, human + `--json`
      output, `mix status`.
      Needs an edge the layering does not have yet: `mixengine-cli -> mixengine-platform`, for
      `ipc::Connection`. Adding it to `ALLOWED_EDGES` in `workspace_layering.rs` is the
      architectural decision, and it is a narrow one — the client uses the transport and nothing
      else in that crate, and business logic in a client stays forbidden either way.
      Decide there whether the client verifies the *server*. On Unix it is already answered: the
      socket lives in `run/`, which is owner-only, so no other account can put a file there. Windows
      has no such directory — the pipe namespace is flat and world-writable, so another local
      account can create `\\.\pipe\mixengine.<our-sid>.<fingerprint>` while no daemon is running and
      collect whatever a client sends it. `FILE_FLAG_FIRST_PIPE_INSTANCE` (T7) turns that into a
      daemon that refuses to start rather than a silent interception, which is most of the value.
      Closing it properly means the client reading the pipe's owner SID, and that needs
      `READ_CONTROL` on the handle, which tokio's `ClientOptions` cannot ask for — so it means a raw
      `CreateFileW` and building the `NamedPipeClient` from the handle. Worth doing only if the
      answer to "who else has an account on this machine" is ever anything but "nobody".
- [ ] **T11** Test harness: per-test `TempDir` home, `mock::Host` with operation recording,
      `fakeservice` fixture binary, `MockRegistry`.

**Milestone M0** — `mix status` prints a healthy daemon on all three OSes in CI.

---

## Phase 1 — Process supervision
*Goal: we can run and babysit arbitrary programs correctly. Everything later is built on this.*

- [ ] **T12** `ServiceSpec`, `ReadyCheck`, `HealthCheck`, `RestartPolicy`, `StopBehaviour` types.
- [ ] **T13** Spawn with process groups: Job Object (Windows), `setsid` + `PR_SET_PDEATHSIG` (Unix);
      no orphans when the daemon dies. **(P)**
- [ ] **T14** State machine + persistence + `ServiceStateChanged` events; `Degraded` vs `Failed`.
      The first `sqlx::query!` in the workspace lands here, so it brings the offline data with it:
      committed `.sqlx/`, `cargo sqlx prepare --check` in CI, and no `DATABASE_URL` needed to build
      (see T6).
- [ ] **T15** Ready/health polling, restart backoff, crash-loop cutoff with the last 200 log lines
      attached to the failure reason.
- [ ] **T16** Log capture: line splitting, per-service files, size rotation, in-memory ring buffer,
      `LogLine` events, `GET /logs/{id}?follow=1`.
- [ ] **T17** Dependency DAG start/stop ordering; cycle detection at spec-build time.
- [ ] **T18** Crash recovery: PID + start-time adoption, stale socket/pidfile cleanup on daemon boot.
- [ ] **T19** `service.*` RPC surface + `mix service start|stop|restart|status|logs`.

**Milestone M1** — kill the daemon mid-run; on restart it adopts what survived and cleans what did
not. Proven by tests against `fakeservice` on all three OSes.

---

## Phase 2 — Runtimes
*Goal: multiple PHP/Node/Python/Ruby versions installed and selectable.*

- [ ] **T20** Package index client: fetch, Ed25519 signature verification, 6-hour cache, offline mode.
- [ ] **T21** Download pipeline: resumable download, SHA-256 verification, staging dir, atomic rename,
      rollback on failure, post-install smoke test.
- [ ] **T22** Job system: `jobs` table, `JobProgress`/`JobFinished` events, `job.wait`, cancellation.
- [ ] **T23** `runtime.install|uninstall|list_installed|list_available|set_default` — **PHP first**.
- [ ] **T24** Version resolution (`core::resolve`): flag → `mixengine.toml` → project record → default;
      exact/minor/caret constraints.
- [ ] **T25** Shim binary: name-based dispatch, in-process resolution without IPC, `exec` on Unix /
      Job-Object child on Windows, exit-code and signal passthrough. **(P)**
- [ ] **T26** PATH integration for `<root>/bin`, reversible. **(P)**
- [ ] **T27** Node.js, Python, Ruby support in the same pipeline.
- [ ] **T28** PHP extensions: `conf.d` model, enable/disable, prebuilt extension artifacts, per-pool
      reload.
- [ ] **T29** Shim overhead benchmark in CI (< 15 ms budget).

**Milestone M2** — two PHP versions installed; `php -v` differs between two directories with no shell
hook installed.

---

## Phase 3 — Services
*Goal: web server, databases and caches run with generated config.*

- [ ] **T30** Config generation engine: `minijinja` templates, typed overrides, atomic write,
      no-op-if-identical diffing, validation hook before install.
- [ ] **T31** Caddy integration: global Caddyfile + per-site imports, `caddy validate`, graceful
      reload, admin API health.
- [ ] **T32** php-fpm pools: one service per PHP version, socket/port per pool, `SIGUSR2` reload.
- [ ] **T33** MariaDB: install, `mariadb-install-db` first-run job, random root password in the OS
      keyring, secure defaults, dev-tuned `my.cnf`. **(P)**
- [ ] **T34** PostgreSQL: `initdb`, `pg_hba` local-only, superuser creation.
- [ ] **T35** Redis + Memcached with dev-tuned config.
- [ ] **T36** Multiple instances of one service (`mariadb@main`, `mariadb@legacy`) with independent
      ports and data dirs.
- [ ] **T37** Nginx as the alternative front end; parity test suite running both generators.
- [ ] **T38** Port conflict diagnosis: report the owning process name, not just `EADDRINUSE`. **(P)**

**Milestone M3** — `mix service start caddy mariadb redis` → all healthy in under 10 s warm.

---

## Phase 4 — Sites, domains and on-demand elevation
*Goal: `http://blog.test` works, and creating a site prompts for nothing.*

Design: [ADR 0005](../decisions/0005-on-demand-elevation.md). Nothing here installs a persistent
root process.

- [ ] **T39** Project & site model: create/import/update/delete, doc root, site kinds
      (`php-fpm`, `static`, `reverse-proxy`, `node-app`), `mixengine.toml` read/write.
- [ ] **T40** **`mixengine-elevate`**: one-shot binary, typed request/response over files, self
      validation, atomic writes under lock, root-owned audit log, distinct "user declined" exit code. **(P)**
- [ ] **T40a** `Elevation` trait: `ShellExecuteEx`/`runas`, osascript `with administrator privileges`,
      `pkexec` — **including polkit-agent detection and the manual-command fallback on Linux**. **(P)**
- [ ] **T40b** Elevation queue in the daemon: batch pending ops into one invocation,
      `ElevationRequired` event, decline → degraded mode with a pending list. Test: no code path
      elevates in a loop.
- [ ] **T41** `PrivilegedOp::HostsApply` — marker-block editing with atomic write, locking, and the
      "unrelated lines survive" regression test. **(P)**
- [ ] **T42** `PortAccess`: no-op on Windows, pf anchor redirect on macOS, `setcap`/nftables on Linux,
      plus **re-probe after every app update** (setcap is lost when the binary is replaced). **(P)**
- [ ] **T43** Site → config → reload end-to-end; `site.start|stop`, idempotent re-runs.
- [ ] **T44** Built-in DNS server (`hickory`): bind **5353** on macOS/Linux and **53** on Windows,
      wildcard answers for managed TLDs, upstream forwarding, loopback-only recursion, port-in-use
      detection with the owning process reported.
- [ ] **T45** Resolver wiring per OS with a custom port: `/etc/resolver` + `port`,
      `resolvectl dns …:5353` / dnsmasq `#5353`, NRPT (port 53) — TLD-scoped only, never global. **(P)**
- [ ] **T46** `domain.*` RPC + `domain.dns_status` real-lookup diagnostics.
- [ ] **T46a** Hosts-only fallback mode: wildcards disabled, batched hosts prompts, clearly signalled
      in the GUI.
- [ ] **T47** `mix doctor` / `doctor_repair`: reconcile hosts, DNS, resolver, port grant, orphans,
      stale config; flush deferred privileged ops; **detect Windows excluded port ranges**
      (`netsh int ipv4 show excludedportrange`) which look like permission errors but are not.
      Also re-check home permissions via `DirectoryAccess::is_restricted_to_owner` (T3a). **Decide
      there whether to keep `icacls`**: the answer it gives on Windows is narrow — inheritance
      severed, yes or no — because `icacls` prints localised account names and no SIDs, so the
      trustee list cannot be checked. Doing better means `GetNamedSecurityInfoW` +
      `GetSecurityDescriptorControl` (the `SE_DACL_PROTECTED` flag, exactly, no parsing) and
      `GetAce` + `EqualSid` to compare the three trustees, with `SetNamedSecurityInfoW` +
      `SetEntriesInAclW` replacing the apply path for symmetry. That is ~150 lines of `unsafe`
      FFI on `windows-sys`, which this crate is allowed per item — the reason it was not done in
      T3a is that the *apply* path is verified working and the check had no caller yet. If T47
      only reports "inheritance is intact", the swap is still not worth it.

**Milestone M4** — create a site and open `http://blog.test` in a fresh shell on all three OSes with
**zero elevation prompts after first-run setup**; `mix uninstall --dry-run` shows a complete cleanup.

---

## Phase 5 — HTTPS
*Goal: green padlock, automatically, forever.*

- [ ] **T48** Internal CA generation (`rcgen`), key permissions, fingerprint, `cert.ca_status`.
      Note from T3b: `restrict_to_owner` covers directories, not the files in them, and that is only
      safe because `certs/` is stripped of its ACL *before* anything is written into it — an
      inheritable ACE on a parent reaches new files too (`file_inherit`). A key written to a
      directory that has not been restricted yet keeps the inherited ACE for its whole life, since
      nothing revisits it. Keep the order, or restrict the key file itself.
- [ ] **T49** Trust store install/remove per OS, including Linux NSS DBs for Firefox/Chrome —
      **batched with T42 and T45 into the single first-run elevation prompt**. **(P)**
- [ ] **T50** Leaf issuance: 90 days, site SANs, `serverAuth` only, idempotent reuse.
- [ ] **T51** Web server TLS wiring; **disable Caddy's automatic ACME** explicitly.
- [ ] **T52** Renewal scheduler: daily + on-boot check, < 30 days threshold, reload without restart.
- [ ] **T53** `mix cert status` with a live handshake and SAN-mismatch detection; one-click reissue.
- [ ] **T54** `cert.ca_rotate` and complete `ca_uninstall`, verified by enumerating the stores.

**Milestone M5** — `https://blog.test` is trusted in Chrome, Firefox, Safari and Edge on their
platforms; adding a domain keeps the padlock green.

---

## Phase 6 — Desktop GUI
*Goal: the terminal becomes optional.*

- [ ] **T55** Tauri v2 shell + Rust proxy to the daemon socket + SSE relay to the webview.
- [ ] **T56** `ts-rs` binding generation + the CI check that committed bindings are current.
- [ ] **T57** Frontend foundation: Vite, strict TS, TanStack Query, event→invalidation mapping,
      `ui/` primitives, i18n (English + Vietnamese), light/dark.
- [ ] **T58** Dashboard: service tiles, metrics (`metrics.subscribe`, sampling only while subscribed),
      disk usage, recent events.
- [ ] **T59** Sites screen: list, create/edit drawer, open in browser/folder/terminal.
- [ ] **T60** Runtimes screen: installed/available, install jobs with progress, PHP extension toggles.
- [ ] **T61** Services screen: settings forms, rendered config read-only, validation errors,
      credential reveal.
- [ ] **T62** Logs viewer: live tail, filter, search, pause-on-scroll.
- [ ] **T63** Domains & TLS screen: the diagnostic table, CA install/uninstall, per-site reissue.
- [ ] **T64** Elevation UX: first-run setup screen requesting one batched prompt; per-op dialogs
      showing the literal change (the exact hosts lines, the port, the store); a persistent "pending
      permissions" surface after a decline.
- [ ] **T65** Tray/menu-bar item: state, start/stop all, quick-open sites, sharing indicator.
- [ ] **T66** Settings screen + `doctor_repair` surface; "copy diagnostics" on every error.
- [ ] **T67** GUI cold-start benchmark (< 1.5 s) and Playwright E2E for create-site → open.

**Milestone M6** — a user installs, creates a Laravel-shaped site with HTTPS, and never opens a
terminal.

---

## Phase 7 — Efficiency
*Goal: deliver the promise that idle costs nothing.*

- [ ] **T68** `ResourceLimits` per OS: Job Objects, cgroup v2, macOS QoS + watchdog; the GUI shows only
      what the platform really supports. **(P)**
- [ ] **T69** Idle detection (connections, request counters, query counters) and `IdlePolicy`
      shutdown, with per-project "keep warm".
- [ ] **T70** On-demand activation gateway: hold the socket, start the service, wait for ready, proxy
      the first request.
- [ ] **T71** Metrics history: 1 s sampling while subscribed, 24-hour downsampled retention.
- [ ] **T72** CI budgets: idle footprint < 60 MB RSS, cold path < 1.5 s — failing the build on
      regression.
- [ ] **T73** Dev-tuned defaults pass over every service template (buffer pools, memory limits).

**Milestone M7** — after 30 idle minutes only `mixengined` + the web server are running, and the next
request still succeeds within budget.

---

## Phase 8 — Differentiators

- [ ] **T74** LAN sharing: per-site opt-in, rebind, firewall rule (one elevation prompt), LAN URL +
      QR code. **(P)**
- [ ] **T75** mDNS advertisement (`<slug>.mixengine.local`) and CA download endpoint for phones.
- [ ] **T76** Auto-revoke sharing on network change; sharing visible in the tray; the "web ports only"
      enforcement test.
- [ ] **T77** Blueprint manifest, `blueprint.capture`, and plan/`--dry-run` output.
- [ ] **T78** `blueprint.apply` execution with resumable idempotent actions and scoped rollback.
- [ ] **T79** Built-in blueprint gallery (Laravel, WordPress, Symfony, static, Next.js proxy, Django),
      doubling as end-to-end tests.
- [ ] **T80** Extension model: `extension.toml`, the four kinds, scoped tokens and permission
      enforcement.
- [ ] **T81** Extension registry client + install/uninstall/start/stop + GUI store screen.
- [ ] **T82** First extensions: Mailpit (with the `sendmail_path` recipe for every managed PHP),
      phpMyAdmin, Adminer.
- [ ] **T83** **MixDB integration**: detect installed MixDB, "Open in MixDB" on every database service,
      connection handoff with credentials read from the keyring at click time.
- [ ] **T84** MixDB as a `desktop-app` registry entry + a shared keyring naming convention.

**Milestone M8** — capture a project as a blueprint, apply it to a new one, open its database in
MixDB, and test it from a phone — all from the GUI.

---

## Phase 9 — Ship

- [ ] **T85** Installers: NSIS per-user + portable zip, `.dmg`, AppImage/`.deb`/`.rpm`; place
      `mixengine-elevate` in a root-owned directory. **(P)**
- [ ] **T86** Minisign updater keys: generation, CI signing of artifacts, pubkey pinned in the app.
      **No OS code signing** — see [ADR 0005](../decisions/0005-on-demand-elevation.md) and
      [updates.md](../features/updates.md).
- [ ] **T86a** Unsigned-distribution reality check: SmartScreen behaviour across two consecutive
      releases; Defender `HostsFileHijack` heuristic with full protection enabled; Gatekeeper flow on
      macOS 15+. Document the findings in `updates.md`. **(P)**
- [ ] **T87** Complete uninstall path + a clean-VM smoke test proving nothing is left behind.
- [ ] **T88** Auto-update: `latest.json` on GitHub Releases via the stable asset URL (not the API),
      launch check + 24 h interval, silent on failure, consent dialog with notes and size,
      stop → update → relaunch → restore running services, skip/later persisted.
- [ ] **T88a** `mixengine-elevate` update path: excluded from auto-update, own elevation prompt,
      minisign verified **inside** the elevated context, daemon↔elevate protocol negotiation.
- [ ] **T88b** Post-update port-access re-probe (`setcap` is lost when the binary is replaced) and
      re-request if needed. **(P)**
- [ ] **T89** Upgrade test: an old `mixengine.db` migrated by a new binary, in CI.
- [ ] **T90** User documentation site + in-app help; English and Vietnamese.
- [ ] **T91** Crash reporting that is opt-in and contains no project paths or credentials.
- [ ] **T92** Public beta: the packaging pipeline running for all runtimes across six OS/arch targets
      ([../operations/runtime-packaging.md](../operations/runtime-packaging.md)).

**Milestone M9 — v0.1.0.**

---

## Parked (revisit deliberately, do not start early)

- **Buying an Apple Developer ID / Authenticode certificate.** Would remove the Gatekeeper and
  SmartScreen friction and would reopen the persistent-helper option — the two decisions are linked
  ([ADR 0005](../decisions/0005-on-demand-elevation.md)).
- Optional Docker escape hatch for exotic services (see
  [ADR 0003](../decisions/0003-no-container-isolation.md)).
- Remote tunnels (Cloudflare/ngrok) as an extension.
- Team-shared blueprint registries.
- Editor extensions (VS Code / JetBrains) as additional API clients.
- Xdebug one-click profiles and a built-in profiler view.
