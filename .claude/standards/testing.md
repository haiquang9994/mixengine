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
2. **Every test gets its own `MIXENGINE_HOME`** in a `tempfile::TempDir`. No shared fixture
   directory, no leakage between tests, parallel-safe.
3. **`mock::Host` records operations**; assertions are on the recorded sequence
   (`assert_eq!(host.hosts_writes(), [...])`), not on side effects.
4. **Every platform-file mutation has a preservation test**: write unrelated content, apply, roll
   back, assert the unrelated content is byte-identical.
5. **Supervision is tested against `fakeservice`**, never against real MariaDB — see
   [../architecture/process-supervision.md](../architecture/process-supervision.md).
6. **Bug fixes ship with the failing test first.** No exceptions for "obvious" fixes.

## Fixtures

- `tests/fixtures/fakeservice` — configurable child process (slow start, never ready, exit code,
  ignores SIGTERM, spawns orphan).
- `tests/fixtures/fakepackage/` — a tiny tarball/zip with a known SHA-256, used for install flows
  without hitting the network.
- `MockRegistry` — an in-process HTTP server serving a signed index and artifacts.
- **Network access in tests is forbidden** outside of `MockRegistry`; CI runs with network egress
  blocked for the unit/component/integration jobs to enforce it.

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
