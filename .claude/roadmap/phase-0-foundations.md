# Phase 0 — Foundations

*Goal: an empty but real system — daemon starts, CLI talks to it, state persists.*

Part of the [build plan](todo.md). Legend: `[ ]` todo · `[~]` in progress · `[x]` done · **(P)** =
has a platform-layer component and needs verification on Windows + macOS + Linux.

---

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
- [x] **T2b** Find out what the Windows test leg runs *as*, and stop it proving less than it appears
      to. **(P)**
      **Answered: fully elevated.** `whoami /groups` on `windows-latest` reports
      `BUILTIN\Administrators` as `Enabled group, Group owner` at `High Mandatory Level`, under the
      account `runneradmin` — not the UAC-filtered token where that group sits deny-only and grants
      nothing. The worst of the two possibilities, and the reason a name in a cache path was not
      allowed to stand in for it.
      **No assertion in Rust, and no de-escalation, because nothing was proving less than it
      appeared to.** Reading `crates/mixengine-platform/tests/access.rs` before writing a fix is
      what showed it: every Windows claim there is *structural* — the `icacls` listing, the `(I)`
      flag, the count of grants — and a structural claim about an ACL reads identically from any
      account that can open the directory. Not one of them attempts an access a token gets to
      decide, so not one of them depends on this. A fix would have been code answering a problem
      that had not happened.
      **What the task produced instead is the rule for the tests that would have had it**, in
      [testing.md](../standards/testing.md): on Windows, prove exclusion structurally and never by
      trying it, since an elevated process wins the attempt either way — and T40's refusal to run as
      an administrator cannot be proved by a suite that is one. Both land in the `system` job, where
      this token is the enabling condition rather than the hazard: excluding a second account
      requires the privilege to create one.
      The probe stays in `ci.yml` as an assertion that fails if the runner image ever de-escalates,
      because that page now states the answer as fact.
      The Linux leg already takes this seriously: `.github/scripts/test-no-network.sh` tries
      `--map-current-user` before `--map-root-user` so the suite does not see itself as uid 0 and
      quietly invalidate every assertion about file permissions and about refusing to run as root
      (T7, T40). The Windows leg has no equivalent, and a GitHub-hosted runner is understood to run
      under an administrative account — which would mean T3a's ACL assertions, and every later one
      about a home being shut to other accounts, have been passing *for an administrator* on the one
      OS where that work was hardest and where `is_restricted_to_owner` is already narrower than it
      looks.
      **Confirm before designing anything**: one `whoami /groups` step on the Windows runner, read
      once. If it is elevated, the proportionate answer is an assertion rather than a de-escalation —
      a test that says what it is proving, or refuses to run, when the process holds
      `BUILTIN\Administrators` — because dropping privilege on Windows is a token operation with no
      `runuser` to borrow, and building one to host a test suite is a larger thing than the problem it
      would solve.
      **In Phase 0 rather than beside T40**, where the no-root rule first gets code to enforce: the
      assertions this affects landed in T3a and have been running in CI ever since.
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
- [x] **T8** JSON-RPC server over the transport: `daemon.status`, `daemon.version`, `/health`,
      `/events` SSE, panic-to-`internal` catch, request spans.
      Three modules under `mixengine-daemon/src/api/`, split so that each can be tested by itself:
      `http` never decodes a call, `rpc` never sees a header, `events` never sees either. The wire
      types are all new in `mixengine-proto` (`rpc`, `DaemonStatus`, `DaemonVersion`, `Health`,
      `DaemonEvent`, `Timestamp`, `Uptime`), because the CLI at T10 depends on that crate and on
      nothing else.
      **JSON-RPC 2.0 as written, not approximated**, since off-the-shelf clients were the reason
      [daemon-and-ipc.md](../architecture/daemon-and-ipc.md) chose HTTP at all: a validated
      `"jsonrpc":"2.0"` member, batches, notifications, and an *integer* `error.code`. That last one
      is the only real tension with T5, and it is resolved by carrying both — the five reserved
      integers plus `-32000` for everything MixEngine itself refuses, and the stable `ErrorCode`
      string in `error.data.code`, which is what `mix` and the GUI branch on. The message is written
      once, in the standard `message` member, because a hint and a message that both restate it
      would print the same sentence three times in a GUI that renders all of them.
      The one place the spec is easy to get wrong, and was: a notification is a request with no `id`
      **member**, while `"id":null` is a request that must still be answered. `Option<Id>` reads an
      absent member and a null one as the same `None`, so deciding it from the decoded request would
      leave a client that sent `"id":null` waiting forever. The distinction is taken from the
      undecoded JSON before the call is decoded, `Request::is_notification` says out loud that it
      cannot tell, and `Response::success` takes an `Option<Id>` so the answer can echo the null it
      was given rather than improve on it.
      **The HTTP status describes the envelope, never the call.** A method that fails is a `200`
      carrying an `error` member, because the request was delivered, parsed and answered; a `4xx`
      there would make `not_found` on a site indistinguishable from `/rpc` being typed `/rcp`, and
      would make a client branch in two places on one outcome. The statuses that do appear — `204`
      for a body of nothing but notifications (an empty `200` hands zero bytes to a client that
      parses every response), `400`, `404`, `405` with `Allow`, `413` — are all about the envelope,
      and their bodies are the plain `Error` shape rather than a JSON-RPC response, since there is no
      `id` to answer and no method that ran. `413` and `400` are told apart by downcasting to
      `LengthLimitError` rather than guessed at: `Limited` reports the cap and a client that died
      mid-body through the same boxed error, and a `413` naming a limit nobody hit sends whoever
      reads the log looking for the wrong thing. `/health` answers `HEAD` as well as `GET`, since
      that is what a liveness probe reaches for and what HTTP expects of anything answering `GET`;
      `/events` does not, because its whole answer *is* its body.
      The panic catch is a `tokio::spawn` per call whose `JoinError` becomes `internal`, rather than
      `catch_unwind`, which needs a `futures` combinator and an `AssertUnwindSafe` around a future
      that genuinely is not. Letting the *connection* die instead was considered and rejected: the
      client would see a dropped socket and have no way to know whether its request had been carried
      out. This is what `panic = "abort"` in the release profile would defeat, which the workspace
      manifest already says out loud. It is proved by a handler that exists only under `#[cfg(test)]`
      and panics — catching a panic raised anywhere else would prove something about the test.
      `daemon.status` reports only what this build actually knows, with no empty `services`/`sites`
      arrays standing in for concepts that do not exist yet: a client rendering "0 services" before
      Phase 1 is showing a fact nobody established, and adding a field later costs a client nothing
      while removing one costs it a release. `started_at` needed a wire representation for a moment
      and there is no date crate in [rust.md](../standards/rust.md) — `Timestamp` is milliseconds
      since the epoch, signed, which needs no parser and is `new Date(ms)` in the GUI. `uptime` is
      computed from `Instant` and not from it, so "up 3 days" survives a system clock corrected
      while the daemon ran. Both readings are taken on the first line of `main` and passed down as
      `api::Started`, not taken in `Api::new`: creating a home, running the migrations and opening
      SQLite happen in between, and a reading taken afterwards leaves all of it out of `uptime` on
      exactly the start where it takes longest.
      `/events` is the pipe and not the vocabulary. The enum in
      [daemon-and-ipc.md](../architecture/daemon-and-ipc.md) names `ServiceId`, `JobId` and
      `MetricsSample`, none of which exist, so declaring it now would mean inventing identifier
      types before the code that issues them has an opinion, and publishing a contract nothing can
      produce. `DaemonEvent::Resync` is the one variant, and it is the one that belongs to the
      stream itself: the channel is the documented bounded 1024, and a receiver that falls behind is
      told how much it missed rather than buffered. Frames are internally tagged with no SSE
      `event:` line, which gives the GUI one `onmessage` that switches on `type` — and means a
      variant added in a later phase reaches an older client as an object it can ignore rather than
      as an event type it never subscribed to and silently never sees. The body is a
      `stream::unfold` polled by hyper rather than a task writing into a queue, so a client that
      stops reading stops the stream instead of filling a buffer behind it; a 15-second `:` comment
      keeps an idle stream distinguishable from a dead one.
      Three things had to be found rather than designed. `header_read_timeout` **panics** on the
      first connection unless `Builder::timer` is set — hyper is runtime-agnostic and owns no clock
      — which took every integration test from a failure to `IncompleteMessage` and was found by
      letting the daemon's stderr through. `tracing-subscriber` formats a span's fields once and
      *appends* whatever is recorded afterwards, so the `Empty`-then-`record` idiom for the request
      id printed `id=3 id=3`; the id is rendered into the span up front instead. And `Version` must
      deserialise an owned `String` rather than a borrowed `&str`: the body is read into a `Value`
      first — a batch and a single call are told apart before either is decoded — and nothing can
      borrow out of one, so the borrowing version failed on every request that arrived the way they
      all actually arrive.
      Tested against the real binary in `tests/api.rs`, spawned with its own `TempDir` home passed as
      `--home`. Not a choice so much as the shape of the crate: `mixengine-daemon` has no library
      target, so an integration test cannot reach inside it — which is the right constraint, since
      what these prove is exactly what the unit tests cannot, that a daemon started the way a user
      starts one binds the endpoint its home implies and speaks HTTP over it.
      Not here, and both deliberate: `/logs/{service_id}`, which needs a service to have any (T14),
      and `--listen 127.0.0.1:PORT` with its bearer token, which is a second transport and a second
      access-control story for a case nobody has yet. Connections are held in a `JoinSet` and given
      two seconds at shutdown rather than being detached, per the no-task-outlives-shutdown rule in
      [rust.md](../standards/rust.md) — T9 replaces the interrupt that triggers it with the root
      cancellation token, and the set is what that token will have to wait on.
- [x] **T9** Daemon lifecycle: single-instance lock, `--detach`, graceful shutdown with a
      cancellation token. **(P)**
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
      **The flag is inverted from the way this task was written.** `--foreground` is gone and
      `--detach` is the opt-in, because two of the three callers want the foreground: a systemd user
      unit, a launch agent and a Task Scheduler logon task each supervise the process themselves and
      read a fork as a death, and a developer typing the command wants to see the log and press
      Ctrl-C. The one caller that cannot hold the daemon is a client autostarting one (T10), and a
      client is a program that never forgets a flag.
      **A second instance exits 0 after printing the endpoint**, which was a contradiction to
      resolve rather than a line to implement:
      [daemon-and-ipc.md](../architecture/daemon-and-ipc.md) said exit 0, and T7 had already made the
      same situation an `EndpointInUse` failure. Both hold now, because the lock is taken before the
      endpoint is bound and answers the question earlier — a second daemon never reaches the bind, so
      what is left for `EndpointInUse` is a stranger on the endpoint, which is a failure and stays
      one. The exit status is what makes two clients autostarting at the same instant produce one
      daemon and no error message, which is the case it exists for.
      The lock is a held handle and never a pid file: `flock` on Unix, an exclusive share mode on
      Windows, both released by the kernel when the process ends however it ends. A stale lock file
      is therefore not a state the code has to recognise — the file's existence means nothing, only
      the handle does — and one left behind by a machine that lost power costs the next start
      nothing. Three things about it had to be found rather than designed. `flock` rather than
      `fcntl`, because a POSIX record lock belongs to the *process* and is dropped by any `close` of
      any descriptor onto the same file, so reading the holder's pid would silently release a lock it
      never took. The share mode rather than `LockFileEx` on Windows, because a byte-range lock there
      is mandatory rather than advisory: the range holding the pid could then not be read by the
      daemon that wants to name the holder, so the lock would have to be parked at some invented
      offset away from the data — a trick to remember rather than a rule to follow. And the file must
      **not** be truncated at open: opening a file another daemon has flocked succeeds, only the lock
      is refused, so `truncate(true)` would erase a running daemon's pid before we found out it was
      running. It is emptied after the lock is ours instead, and it is not unlinked on release, which
      is what stops two daemons from holding two different files under one name.
      `--detach` re-spawns this same binary without the flag rather than forking. Windows has no
      `fork`, and forking a process that already has a Tokio runtime — several threads, a reactor,
      locks held by threads that do not exist in the child — is a way of producing a daemon that
      hangs on its first `await`. The arguments are rebuilt from what `clap` parsed rather than
      filtered out of `args_os`, which matters most for the home: the child is told the *resolved*
      root, so a relative `--home`, or one that came from the environment, cannot be re-resolved by
      the child against something else. Readiness is a connection to the endpoint and not a
      `GET /health`: what a client needs to know is that there is something to send a request to, and
      dialling proves it without the parent having to speak HTTP — which would have put `hyper`'s
      client in the daemon's dependencies for one probe. The child exiting is polled alongside it,
      because "not up yet" and "gone" are the same silence and only one is worth waiting out — but
      **only a child that exited *unsuccessfully* ends the wait**, and getting that wrong was a real
      bug for a week. A child exits 0 precisely when another daemon already holds the home, and that
      daemon takes the lock *before* it opens SQLite, so between those two moments is a whole set of
      migrations during which the endpoint legitimately does not answer yet. Retrying the endpoint
      once and then giving up — which is what this did at first — turned exactly the case the exit
      status exists for, two clients autostarting at the same instant (T10), into a failure for
      whichever of them lost the race. The deadline is what that case waits on now, and
      `mixengine-daemon/tests/lifecycle.rs` holds the window open deliberately rather than racing for
      it: the test takes the lock itself and never binds anything, which is the state the winner is
      in mid-migration. The parent deliberately does not initialise logging, and it is the *duration*
      that decides it rather than the number of writers: it lives alongside the daemon it started for
      as long as that one takes to come up, which is exactly when the daemon is writing its startup
      lines and may rotate the file out from under a second writer — and one line on stdout is its
      entire output anyway. A daemon that finds the lock taken is the other way round on both counts,
      two lines and gone in milliseconds, and those two lines are worth keeping: "somebody tried to
      start a second daemon at 3am" is what the log is for.
      **The one thing here that had to be found rather than designed**, and it made `--detach`
      useless to its only caller: on Windows the daemon inherited the pipe its parent's stdout was
      on. `CreateProcessW` is called by the standard library with `bInheritHandles = TRUE`, which it
      needs for the child's stdio, and *inheritable* survives inheritance — a handle this process was
      handed arrives still marked inheritable and is passed on again. Redirecting the child's own
      stdio to the null device does not help: the extra copy is not the child's stdout, it is simply
      a handle the child holds, and the writing end of a pipe stays open while anybody holds one. So
      a caller doing what `mix` will do at T10 — `Command::output()`, or any read to end-of-file —
      waits for an EOF that arrives when the *daemon* exits, days later. It was found as a
      `--detach` that returned promptly and a `cargo test` that hung for an hour, and the fix is
      three `SetHandleInformation` calls in `windows/process.rs` clearing `HANDLE_FLAG_INHERIT` on
      the standard handles before the spawn. Unix has the matching hazard and does not need the fix:
      the pipe there *is* fd 1, which `Stdio::null()` replaces, and everything else Rust opens is
      already `CLOEXEC`.
      The flag is cleared for the length of the spawn and put back by a guard `spawn_detached` drops
      straight after it, including when the spawn is what failed. Leaving it cleared costs
      `mixengined --detach` nothing — it exits immediately — but T10's callers go on running, and a
      `mix` that had quietly given up passing its stdio to *every* later child it starts would be a
      surprise nobody asked for. What is left is that a `CreateProcessW` on another thread during the
      spawn may not inherit them, which is a window `bInheritHandles` already opens for every spawn in
      the program.
      **The child is given the home as its working directory**, and that is the fix to the other half
      of the same mistake. A working directory is a reference the OS keeps for the life of the
      process: a daemon that inherited its caller's would stop that directory being renamed or deleted
      on Windows and its filesystem being unmounted on Unix — and the directory a client autostarting
      a daemon is run from is a project folder somebody is working in, which is the last thing worth
      pinning for days. `lifecycle.rs` asserts it by removing the directory `--detach` was started
      from while the daemon runs.
      `--detach` also dials the endpoint once *before* it spawns anything. The case the flag exists
      for is a client that could not reach the daemon, and by the time two of them have both decided
      to autostart one, the second usually arrives to a daemon that is up: starting a process whose
      whole job would be to find the lock taken and exit is a cost with nothing on the other side.
      Signals are a `Signals::listen()` that registers and a `stopped()` that waits, split because
      `select!` rebuilds its futures on every turn — registering inside the loop would tear the
      handlers down and reinstall them continuously, and could lose a signal that arrived in
      between. Failing the start when a handler cannot be installed is deliberate: a daemon that
      cannot be asked to stop is one somebody has to kill, and a shutdown is the wrong moment to find
      that out. Windows takes all five console control events, which are five genuinely different
      ways of being asked to stop; the last three are on a clock — the OS terminates the process a
      few seconds later whatever it is doing — which is why the two-second grace for open connections
      is not merely a taste. A daemon started with `--detach` has no console at all and can receive
      none of them, which is written down in `windows/signal.rs` rather than papered over.
      `GET /events` ends on the root token, which makes the grace period the exception rather than
      the rule: a subscription with nothing to deliver sits in a fifteen-second heartbeat, so a
      shutting-down daemon with a GUI attached used to wait out its whole budget on a stream neither
      end had anything more to say on.
      `tokio-util` is a new dependency, for `CancellationToken` alone — [rust.md](../standards/rust.md)
      names it as the shutdown path every spawned task hangs off, and it is the tokio project's own
      crate. Its `sync` module needs no feature, so `default-features = false` leaves the codec, io
      and net halves out of the build.
      Not covered by a test, for a reason rather than by omission: Windows console control events.
      `GenerateConsoleCtrlEvent` addresses a process *group*, so the event would reach `cargo test`
      and every test binary sharing that console, and a test that terminates the runner proves
      nothing. The Unix half raises a real `SIGTERM` at itself in
      `mixengine-platform/tests/signal.rs`; what covers both is
      `mixengine-daemon/tests/lifecycle.rs`, which starts real daemons and stops them from outside.
      That file carries the one `#[cfg]` outside `mixengine-platform` in this workspace — a
      `stop(pid)` helper — because nothing in the product stops a process by pid yet. It belongs to
      the supervisor when T15 arrives.
- [ ] **T9a** `daemon.shutdown`: the RPC that cancels the root token, so `mix daemon stop` does not
      have to find a pid first. Deferred from T9 deliberately, because the method's real shape is
      "stop every supervised service in reverse dependency order, then stop", and there is no service
      to stop before T13. What exists already is the token it cancels and the arm of the accept loop
      that is waiting on it.
      **It also owns the total shutdown budget, which nothing owns today.** T19 put
      `Registry::shut_down` in front of the daemon's two-second grace for clients with no bound of
      its own, and `mixengine_platform::signal` documents Windows giving a console handler about five
      seconds before it terminates the process regardless — which `SHUTDOWN_GRACE` was sized to fit
      inside. It still fits, for two reasons that are both accidents: `CAN_ASK_TO_STOP` is false on
      Windows so no grace is spent asking a service to leave ([ADR
      0008](../decisions/0008-no-signal-stop-on-windows.md)), and the runners are already stopping
      concurrently by the time `shut_down` waits, so the cost is one `FLUSH` — about two seconds —
      rather than the sum. T15a's `StopBehaviour::Command` is what breaks it, first at T33's
      `mariadb-admin shutdown`, and a daemon terminated mid-shutdown leaves rows claiming `stopping`
      and skips the WAL checkpoint `Store::close` exists for. Not urgent because the console ceiling
      reaches only a *foreground* daemon — a `--detach`ed one has no console for an event to arrive
      on — but the cap belongs here, on the total and not on each service: whatever
      `daemon.shutdown` allows, minus what it has already spent.
- [x] **T10** CLI skeleton: `clap` tree, transport client, daemon autostart-on-connect, human + `--json`
      output, `mix status`.
      The edge this needed is in `ALLOWED_EDGES`, and it is `mixengine-cli -> mixengine-platform`
      for two things rather than one: `ipc::Connection`, and `HomeDirs` for the default home.
      **`mixengine-core` is deliberately still not an edge**, which is what shaped `cli/home.rs`.
      `core` carries `sqlx`, and linking a bundled SQLite into the binary that has to start while
      somebody is waiting at a prompt — to learn that `run/` sits under the root — is not a trade
      worth making. So the client restates two rules `mixengine_core::Paths` owns: an override wins
      and the result is made absolute, and the endpoint belongs to `<root>/run`. The second is safe
      to duplicate for a reason rather than by luck — `Paths::new` passes `None` for `run`
      deliberately, so `[paths]` cannot move the one directory the lock and the endpoint must agree
      on — and `mixengine-cli/tests/status.rs` starts a real daemon against a real client to keep
      the two answers together. Nothing else would notice them drifting: a `mix` that looked in the
      wrong place would silently autostart a second daemon, forever.
      **The client verifies the server on neither system, and the Windows half of that is a
      deferral rather than an answer.** On Unix it is closed already — the socket lives in `run/`,
      which is `0700`, so no other account can put a file there. Windows has no such directory: the
      pipe namespace is flat and world-writable, so another local account can create
      `\\.\pipe\mixengine.<our-sid>.<fingerprint>` while no daemon is running and collect whatever
      a client sends it. `FILE_FLAG_FIRST_PIPE_INSTANCE` (T7) turns that into a daemon that refuses
      to start rather than a silent interception, which is most of the value and is why this is not
      urgent. Closing it properly means the client reading the pipe's owner SID, which needs
      `READ_CONTROL` on the handle, which tokio's `ClientOptions` cannot ask for — so a raw
      `CreateFileW` and a `NamedPipeClient` built from the handle. Worth doing when the answer to
      "who else has an account on this machine" is ever anything but "nobody"; it belongs with T47's
      `mix doctor` checks, where the rest of "is this home still shut to other accounts" lives.
      **Failures are the wire error, always** — `mixengine_proto::Error` whether the daemon refused
      the call or `mix` never reached one, so `--json` hands a script the same object with the same
      `code` either way and nothing branches on which side of the socket produced it. That is what
      pulled `chain` out of the daemon's boundary and into `mixengine_proto::flatten`: two binaries
      now build that message, and the shape of it is a property of the wire error rather than of
      either one. What did *not* move is the part that differs — `cli/error.rs` classifies only the
      handful of platform failures a client can meet, and its advice is a client's. A refused
      endpoint means "another daemon is already running" to the daemon that could not bind it and
      "this home belongs to somebody else" to the client that could not dial it.
      The handshake is `daemon.version` before anything else, as
      [`DaemonVersion`](../../crates/mixengine-proto/src/daemon.rs) says it should be, and it
      happens before there is a `Client` — so "connected" and "speaks our protocol" are one state
      and nothing can hold one of these and still have to remember to check. A round trip on a local
      socket is microseconds, and it is the difference between "these two binaries are from
      different releases" and "the daemon said something unreadable".
      `--no-autostart` is the one flag T10 was not written with. `mix` starting a daemon is what
      makes the first command a person types work, but a monitoring check asking whether MixEngine
      is running must not be the thing that installs it — so the flag makes that run answer instead,
      and `tests/status.rs` asserts the home is still empty afterwards. The daemon binary is found
      at `MIXENGINE_DAEMON_BIN`, then next to `mix`, then on `PATH` by bare name handed to the OS.
      Next-to-`mix` before `PATH` matters most in development: a `target/debug/mix` that autostarted
      the packaged daemon already on the machine would be a confusing afternoon.
      **One thing had to be found rather than designed, and it is T9's hazard one process further
      out.** T9 stopped a detached daemon from inheriting the pipe its caller's stdout was on, and
      fixed it inside `spawn_detached` — which is one copy too late for `mix`. Inheritance on
      Windows is transitive: `mix`'s own stdout reaches `mixengined --detach` the moment it is
      spawned, and that process passes it on to the daemon before `spawn_detached` gets a say, so
      whatever is reading `mix` waits for an end-of-file that arrives when the daemon exits. Found
      the same way T9's was — a `mix status` that returned promptly and a `cargo test` that ran for
      ten minutes without finishing. Redirecting the child's stdio does not help, for the reason
      `windows/process.rs` already gives. The fix is `process::hide_stdio_from_children`, the handle
      half of `detach` exposed on its own: every process in a chain like that has to decline to pass
      its own handles on, and the guard is held across the spawn and dropped straight after, so a
      `mix` that later starts an ordinary child hands it stdio as usual. A no-op on Unix, where only
      the three standard descriptors cross an `exec`.
      Also settled, and small: an empty `--home` never reaches `resolve_root` because `clap` refuses
      the value with its own usage exit code, which `tests/status.rs` pins — the guard `core` and
      the client both keep behind it stays, because treating an empty override as "not given" would
      point a sandbox run at the real install. `mix status` renders no colour and no timestamp: the
      first because these lines are pasted into bug reports, the second because formatting
      `started_at` would mean a date crate that [rust.md](../standards/rust.md) does not name, and
      `uptime` answers the question anybody was asking.
      Not here, deliberately: `mix daemon stop`, which needs T9a, and every other namespace, which
      needs something to talk about. The transport client already takes `params`, so the next
      command is a `clap` variant and a rendering.
- [x] **T11** Test harness: per-test `TempDir` home, `mock::Host` with operation recording,
      `fakeservice` fixture binary, `MockRegistry`.
      `crates/mixengine-testkit`, an eighth workspace member that is a **dev-dependency and never
      anything else**. That is a rule rather than a habit: `workspace_layering.rs` now reads the
      *kind* of each edge, exempts dev-dependencies from the direction rules — a test may use
      whatever it needs — and refuses this one crate anywhere else. Checked by listing it as an
      ordinary dependency of `mixengine-cli` and watching the test fail, rather than trusted.
      A crate rather than the `tests/fixtures/` directory
      [testing.md](../standards/testing.md) named, and the reason is the same one
      `crates/mixengine-cli/tests/status.rs` already had to write a paragraph about:
      `CARGO_BIN_EXE_<name>` only reaches binaries of the package the test is in, so a fixture
      binary four crates share is either a package of its own or a path every one of them hunts for
      on disk. `FakeService::program` still hunts — `target/<profile>/deps/` first, then the profile
      directory above it — because that env var is set for integration tests, not for the library
      they link against; what the crate buys is that the hunting is written once and says
      `cargo test --workspace` when it fails.
      **The `fakeservice` binary contains no `#[cfg]`**, which was the constraint worth designing
      to. Ignoring a
      request to stop and leaving a detached child behind are both things the two families of OS do
      differently, and both are reached through `mixengine-platform` — `Signals::listen` and
      `spawn_detached`. So `fakeservice` is a second user of the daemon's own code rather than a
      second answer to it, and the orphan it leaves does not hold a copy of its parent's stdout: T9
      and T10's hazard, one process further out, and the test times the parent's end-of-file so that
      regressing it fails rather than merely taking a minute.
      The one OS-dependent *body* that remains is `stop`/`try_stop`, moved here out of
      `lifecycle.rs` and `status.rs`, which had a copy each. It stays a `#[cfg]` because nothing in
      the *product* stops a process by pid until T15 — and it is affordable here precisely because
      this crate ships to nobody. Two tests in `tests/fakeservice.rs` are `#[cfg(unix)]` as well,
      which is a different kind of thing: gating a test says where a claim is checkable, and the
      Windows half of that one arrives with the supervisor sending an event to a group it owns.
      `Home` restates three things `mixengine_core::Paths` owns (that `run/` is directly under the
      root, the lock file's name, and `logs/daemon.log`) rather than depending on `core`, for the
      reason `mixengine-cli/src/home.rs` gives: `core` carries `sqlx`, and a test binary has no
      business bundling SQLite to find a socket. What is new is that the answers are now held
      together deliberately — `the_fixture_and_the_daemon_agree_on_the_paths_it_restates` in
      `lifecycle.rs` is the one place both sides exist at once, and every other test in that file
      rests on it. The log is the one that needed the test rather than merely deserving it:
      `Paths::new` refuses to let a `[paths]` override move `run/`, so the first two cannot drift by
      accident, while `logs/` has no such guard and a fixture reading the wrong file would turn every
      `wait_until_daemon_log_says` into a thirty-second timeout blaming the daemon.
      **`Running` drains both pipes from the moment it spawns**, on a thread each, rather than
      leaving it to a `wait_with_output` at the end. `wait_with_output` reads too, but only once it
      is called, and the tests Phase 1 will write hold the handle — polling `still_running`, waiting
      on the supervisor — for as long as the case takes. A pipe holds tens of kilobytes; past that a
      `--log-every` fixture blocks on its next line and never reaches its `--exit-after`, which reads
      as a supervisor bug that is not one. Measured before it was believed: 5 774 lines / 131 KB
      through the pipe across a 45-second run, against a Windows buffer of 64 KB.
      Each reader now fills a shared buffer a block at a time rather than a local one through a
      `read_to_end`, which is what `Running::wait_for_stdout` needed: `read_to_end` hands nothing
      over until the stream closes, and the question a test has is about a service that is still
      running. **CI found the reason it had to exist**, on the two Unix runners and not on the
      Windows one that had run the suite locally: `a_service_can_be_told_to_ignore_being_asked_to_stop`
      signalled the process it had just spawned, and a `SIGTERM` that arrives before
      `Signals::listen` has returned ends it through the default disposition — a service that was
      never asked anything, failing as though it had ignored nothing. Both `#[cfg(unix)]` tests wait
      for `READY_LINE` first, which the program writes only once the handlers are installed; the
      sibling test needed the same wait for the opposite reason, having been able to pass with no
      handlers at all.
      **`mock::Host` needed nothing.** The recording it was to gain arrived with T3a, which is where
      the first mutating capability did; extracting a generic recorder now would be inventing the
      shape the second one needs before it exists, and the four `Mutex<Vec<_>>` lines it would save
      are not a design. Written down in [testing.md](../standards/testing.md) so the next capability
      knows it is expected to bring its own.
      **`MockRegistry` is deliberately not here**, and neither is `fakepackage`. Both serve
      installing a runtime, which is T20 onwards; a signed index format invented now would be a
      contract nothing produces and nothing reads — the same call T5 made about four unused error
      codes and T8 about the event enum.

**Milestone M0 — reached.** `mix status` prints a healthy daemon on all three OSes in CI:
`crates/mixengine-cli/tests/status.rs` starts a daemon over the real endpoint and asserts what it
prints, and the `test` matrix ran it green on `ubuntu-latest`, `windows-latest` and `macos-latest`.
The Windows third of it runs elevated — T2b above, where what that does and does not change is
written down.

---

Next: [Phase 1 — Process supervision](phase-1-process-supervision.md)
