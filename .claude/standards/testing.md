# Testing strategy

MixEngine mutates the user's machine. Tests exist so we can change it confidently — and so we never
ship a regression that eats someone's hosts file.

## Layers

| Layer | Scope | Speed | Where |
| --- | --- | --- | --- |
| Unit | pure logic: version resolution, config rendering, hosts diffing, blueprint planning | ms | next to the code (`#[cfg(test)]`) |
| Component | one subsystem with mocked neighbours: supervisor + fake service, daemon API + mock platform + temp DB | < 1 s | `crates/*/tests/` |
| Integration | real daemon, real SQLite, mock platform, fake binaries | seconds | `crates/mixengine-daemon/tests/` |
| System | real runtimes/services on a real OS, per-platform CI runner | minutes | `tests/system/`, `#[ignore]` by default |
| E2E (GUI) | Playwright against a daemon with mock platform | minutes | `apps/desktop/e2e/` |

Most tests should be unit or component. System tests are few, chosen for what only they can prove.

## Mandatory rules

1. **No test touches the real hosts file, trust store, resolver config or port 53/80/443** unless it
   is a system test explicitly marked `#[ignore]` and gated on `MIXENGINE_SYSTEM_TESTS=1`.
2. **Every test gets its own `MIXENGINE_HOME`** in a `tempfile::TempDir`, **passed as an argument,
   never through the environment**. `std::env::set_var` is `unsafe` in edition 2024 and
   process-global regardless, so two tests in the same binary would rewrite each other's home. The
   environment is read once, at `main`; everything below takes the path it was given.
3. **`mock::Host` records operations**; assertions are on the recorded sequence
   (`assert_eq!(host.restricted(), [...])`, and one like it per capability), not on side effects.
   It grows one recorder per
   capability, added by the task that adds the capability — there is no generic recording machinery,
   because a shape invented before the second user is a guess about what the second user needs.
4. **Every platform-file mutation has a preservation test**: write unrelated content, apply, roll
   back, assert the unrelated content is byte-identical.
5. **Supervision is tested against `fakeservice`**, never against real MariaDB — see
   [../architecture/process-supervision.md](../architecture/process-supervision.md).
6. **Bug fixes ship with the failing test first.** No exceptions for "obvious" fixes.

## Fixtures

They live in **`crates/mixengine-testkit`**, a workspace member that is a **dev-dependency and never
anything else** — `crates/mixengine-proto/tests/workspace_layering.rs` fails the build if any crate
lists it outside `[dev-dependencies]`, so nothing in it can reach `mixengined`, `mix` or
`mixengine-elevate`. That is also what makes it the right home for the one OS-dependent body outside
`mixengine-platform`: stopping a process by pid, which no product code does yet (T15). (Gating a
*test* to the systems it can run on is a `#[cfg]` too and is not the same thing — it says where a
claim is checkable, not how it is answered.)

A crate rather than the `tests/fixtures/` directory this document originally named, for a reason
worth knowing before moving it back: `CARGO_BIN_EXE_<name>` only reaches binaries of the package the
test itself is in, so a fixture binary shared by four crates has to be a package of its own or be
hunted for on disk by every one of them. The bill for that is `clap`, which only the binary needs but
which every suite that links the library compiles anyway: cargo has no per-target dependencies, and
putting it behind a feature with `required-features` would stop `cargo test --workspace` from
building the binary at all — the one thing `FakeService::program` needs to be true.

- `mixengine_testkit::Home` — a `TempDir` home and the endpoint it implies, with the waits that go
  with driving a real daemon. It deliberately restates three of `mixengine_core::Paths`' answers —
  `run/`, the lock file's name and `logs/daemon.log` — rather than depending on `mixengine-core`, and
  `the_fixture_and_the_daemon_agree_on_the_paths_it_restates` in
  `crates/mixengine-daemon/tests/lifecycle.rs` holds all of them against `Paths` at once. The log is
  the one that needs the test: `Paths::new` refuses to let a `[paths]` override move `run/`, so the
  first two cannot drift by accident, while `logs/` has no such guard.
- `mixengine_testkit::FakeService` + the `fakeservice` binary — the configurable child process
  (slow start, never ready, exit code, ignores a request to stop, leaves an orphan). Every mode is
  covered by `crates/mixengine-testkit/tests/fakeservice.rs`: a fixture that quietly stopped
  honouring `--never-ready` would turn a ready-timeout test into one that passes for the wrong
  reason. Two combinations the builder accepts and the program then does not: `never_ready()` with
  `ready_after()`, which `clap` refuses as conflicting, and `exit_code()` without `exit_after()`,
  which is simply never reached. Both are worth a `requires` the first time one of them costs
  somebody an afternoon.
  `FakeService::spawn` returns a `Running` that is **already draining both pipes**, one thread each.
  Do not replace that with a `wait_with_output` at the end: it reads only from the moment it is
  called, and a test that holds the handle while it polls `still_running` or waits on the supervisor
  is nobody draining them until then. A pipe holds tens of kilobytes, after which a `log_every`
  fixture blocks on its next line and never reaches its `exit_after` — a hang that looks like a
  supervisor bug and is not one.
  **A test that signals a service it has just spawned must first wait for `READY_LINE`** through
  `Running::wait_for_stdout`. A spawn returns as soon as the OS has a process, which is before that
  process has parsed its arguments and installed its stop handlers; a `SIGTERM` arriving in that
  window ends it through the default disposition, and a fixture told to ignore a stop then looks like
  one that honoured it. Waiting for the line is what rules the window out, because the program writes
  it only after `Signals::listen` has returned.
- `mixengine_testkit::{stop, try_stop}` — asking a process this test did not start to go away.
  **Not a liveness check on Unix.** `kill` succeeds against a zombie, so `try_stop` answers "there is
  still a pid here", not "there is still a process here". It is sound for the case it exists for — a
  process this test is not the parent of, which nothing in this test can leave unreaped — and wrong
  for anything a test spawned itself and is still holding, where the handle is the answer.
- **Network access in tests is forbidden** outside of `MockRegistry`; CI runs with network egress
  blocked for the unit/component/integration jobs to enforce it.

Not written yet, and deliberately waiting for their first caller rather than being invented ahead of
it — both arrive with runtime installation in Phase 2:

- `fakepackage` — a tiny tarball/zip with a known SHA-256, for install flows without the network.
- `MockRegistry` — an in-process HTTP server serving a signed index and artifacts.

## Cross-platform coverage

CI runs the full unit/component/integration suite on `windows-latest`, `macos-latest` and
`ubuntu-latest`. A PR touching `mixengine-platform` additionally runs the system suite on all three.
"Works on my machine" is not a merge criterion here — the platform layer is the riskiest code we own.

## Performance guards

Benchmarked in CI with a budget that fails the build:

- shim overhead < 15 ms ([../features/runtime-versions.md](../features/runtime-versions.md))
- idle footprint < 60 MB RSS, cold path < 1.5 s
  ([../features/resource-isolation.md](../features/resource-isolation.md))
- GUI cold start < 1.5 s ([../features/gui.md](../features/gui.md))

## Coverage

Tracked, not worshipped. The bar is on the code that can damage a machine: `mixengine-platform` and
`mixengine-elevate` need branch coverage on every failure path, including the rollback ones and the
"user declined" path.
