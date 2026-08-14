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

- `mixengine_testkit::MockRegistry` — a signed package index over a real loopback socket (T20). It
  **generates its own minisign keypair** and signs with the real `minisign` crate, which is what
  forces the product's public key to be injectable: a verification path switched off for tests is a
  verification path nothing checks. It can also serve a document its signature does not cover, and
  answer `503` on demand, so the refusal and offline paths are exercised rather than assumed.
  `minisign` (the signing half) is a dependency of this crate and must never be one of anything
  shipped — a binary holding a signing key would put one on every user's machine.
  From T21 it also **serves artifacts**, and does so as a server a download can be resumed from:
  `publish_asset` puts bytes at a path, `cut_next_response_after` ends one response early the way a
  dropped connection does, and `asset_ranges` is what a test asserts a *resume* on rather than
  "it arrived eventually", which is equally true of a client that downloaded the file twice.
- `mixengine_testkit::FakePackage` — the fixture this page named before it existed: a real archive
  in each of the three shapes the publishing pipeline produces (`.zip`, `.tar.gz`, `.tar.zst`), with
  the SHA-256 stated rather than asked of the code under test. `Packing::ALL` is what makes an
  install test cover all three on every runner instead of only the one its platform uses.
  **The compressors are deliberately not the ones the product decompresses with** — `zstd` here
  against `ruzstd` there, on the same principle as `minisign`/`minisign-verify` — because a fixture
  built by the implementation that reads it proves only that the implementation agrees with itself.
  `executable()` packs the `fakeservice` binary, and packs a *real program* rather than a script
  because Windows cannot spawn a `.bat` without a shell: the post-install check would then be
  exercised on two platforms and skipped on the third.

## Cross-platform coverage

CI runs the full unit/component/integration suite on `windows-latest`, `macos-latest` and
`ubuntu-latest`. A PR touching `mixengine-platform` additionally runs the system suite on all three.
"Works on my machine" is not a merge criterion here — the platform layer is the riskiest code we own.

## The machine the suite runs on is part of the test

Two properties of the host change what a green suite *means* on Windows, silently, and both have
already been met rather than imagined.

**Privilege.** A test asserting that a directory is shut to other accounts, or that something refuses
to run as root, proves nothing when the process running it is elevated: it passes for a reason that
will not exist on a user's machine. `.github/scripts/test-no-network.sh` already reasons this way on
Linux, trying `--map-current-user` before `--map-root-user` so the suite does not see itself as uid 0
and quietly invalidate every assertion about file permissions (T7, T40).

**The Windows leg is elevated, measured rather than assumed** (T2b). `whoami /groups` on
`windows-latest` reports `BUILTIN\Administrators` as `Enabled group, Group owner` at
`High Mandatory Level` — a full token, not the UAC-filtered one an administrator ordinarily carries,
where that group is present *deny-only* and grants nothing. The account is `runneradmin`. The CI step
that prints this fails the job if it ever stops being true, because the two paragraphs below depend
on it being true.

It invalidates nothing asserted today, and that is a property of how those tests are written rather
than luck. `crates/mixengine-platform/tests/access.rs` proves its Windows claims by *reading the
DACL* — the `icacls` listing, the `(I)` flag, the number of grants — and never by attempting an
access that a token gets to decide. A structural claim about an ACL reads the same from any account
that can open the directory at all, so those tests mean on a user's machine what they mean here.

What it invalidates is the next test written the obvious way. **On Windows, prove exclusion
structurally, never by trying it**: an elevated process opening a file it was supposed to be shut out
of succeeds, so the test expecting a denial fails, and its mirror image — one expecting success —
passes for a privilege the user will not have. T40's refusal to run as an administrator is the same
trap from the other side and cannot be proved by a suite that *is* one. Both belong in the `system`
job, where this token stops being a hazard and becomes the enabling condition: creating a second
account to be excluded by needs exactly the privilege this leg turns out to hold.

**Application Control.** On a Windows 11 machine with Smart App Control enforced, freshly built test
binaries are refused at *image load* — `os error 4551`, "An Application Control policy has blocked
this file" — and cargo reports that as a target that failed, not as an environment that would not run
it. The judgement is per file, on signature and cloud reputation.

**Observed once, and it did not persist.** The same two binaries ran unchanged a few hours later,
directly and under cargo, and freshly built release binaries whose hashes had never existed were not
refused at all — consistent with a reputation lookup that had not answered yet the first time a
file was seen, though the mechanism is unconfirmed. So the first response to `4551` is to run it
again; concluding anything from a single refusal is premature, and this note originally did exactly
that.

What holds either way: **Microsoft Defender path exclusions do not apply**, because they configure a
different subsystem and a directory Defender has been told to ignore is still policed by Code
Integrity — and no change to this codebase avoids it, since the verdict is on the file rather than on
how it was launched. If refusals ever do start persisting, the three options are turning SAC off on
the development machine (**a one-way door** — it cannot be re-enabled without reinstalling Windows),
developing in a VM with it off, or treating CI as the authority for whichever targets are refused
that day. The same mechanism is a *product* problem, several sizes larger — a user's first launch is
exactly the first-seen case — and is measured by
[T41a](../roadmap/phase-4-sites-and-elevation.md); the evidence is recorded in
[../features/updates.md](../features/updates.md).

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
