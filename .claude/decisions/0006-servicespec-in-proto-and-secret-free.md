# 0006. `ServiceSpec` lives in `mixengine-proto` and never carries a secret

**Status**: Accepted
**Date**: 2026-08-11

## Context

Roadmap task **T12** defines `ServiceSpec`, `ReadyCheck`, `HealthCheck`, `RestartPolicy` and
`StopBehaviour`, and doing so forces a question the documentation had left in two incompatible
states:

- [../architecture/process-supervision.md](../architecture/process-supervision.md) said the
  supervisor "consumes a `ServiceSpec` produced by `mixengine-core`".
- [../architecture/overview.md](../architecture/overview.md) and the `ALLOWED_EDGES` table in
  `crates/mixengine-proto/tests/workspace_layering.rs` make `core` and `supervisor` siblings —
  neither may depend on the other.

Both cannot be true while the type lives in either crate. Nothing had been implemented yet, so this
is an undecided question rather than a mistake, but every later phase builds on the answer.

Four constraints from elsewhere in the specification narrow it:

1. [../architecture/daemon-and-ipc.md](../architecture/daemon-and-ipc.md) states flatly: **all types
   are defined in `mixengine-proto`**.
2. The GUI's Services screen edits limits, autostart and idle timeout
   ([../features/client-surface.md](../features/client-surface.md)), and its TypeScript types are
   generated from `mixengine-proto` with `ts-rs`. `ResourceLimits` and `IdlePolicy` therefore have to be proto
   types; that part was never actually open.
3. An extension manifest declares a service in TOML — `program`, `args`,
   `ready = { tcp = …, timeout = "10s" }` ([../features/extensions.md](../features/extensions.md)) —
   so the shape must deserialize from third-party text, and is persisted in
   `extensions.manifest_toml`.
4. `services.limits_json` and `services.idle_minutes` are columns
   ([../architecture/data-model.md](../architecture/data-model.md)): the policy types are stored, not
   merely passed.

The one argument against putting the whole thing in proto is that `ServiceSpec.env` would carry
MariaDB's generated root password, and proto types derive `Serialize`.

That argument is real but its usual remedy is wrong. Every large system that has met it solved it by
keeping the secret out of the spec, not by moving the spec: Kubernetes puts
`env.valueFrom.secretKeyRef` in `PodSpec` rather than the password; systemd added
`LoadCredential=`/`SetCredential=` precisely because `Environment=` leaks through `systemctl show`;
Docker grew BuildKit and Swarm secrets because `docker inspect` prints environment variables. Moving
the struct to another crate hides the hazard from the wire while leaving it in the database, the
logs and the diagnostics bundle.

This project had already written the same instinct down without connecting it to supervision: the
`Keyring` trait in [../architecture/platform-abstraction.md](../architecture/platform-abstraction.md),
"random root password in OS keyring" in [../features/services.md](../features/services.md), and
"never write a password into a URL that lands in a shell history or a log" in
[../features/extensions.md](../features/extensions.md).

## Decision

**`mixengine-proto` owns the whole service vocabulary**, `ServiceSpec` included, alongside
`ServiceId`, `ReadyCheck`, `HealthCheck`, `RestartPolicy`, `Backoff`, `StopBehaviour`,
`ResourceLimits`, `IdlePolicy` and `LogPolicy`. `core` builds a spec, `supervisor` runs one, and both
look downward at `proto` — so `ALLOWED_EDGES` does not change and the sibling graph
`overview.md` draws stays exactly as drawn. The sentence in `process-supervision.md` is corrected to
match.

**A spec cannot express a secret by value.** Environment entries are:

```rust
pub enum EnvValue {
    Literal(String),                          // non-secret by contract
    Keyring { service: String, key: String }, // resolved at spawn time, inside the supervisor
}
```

The supervisor resolves `Keyring` entries through the platform `Keyring` capability at the moment it
spawns the child, and the resolved value exists only in the `Command` it is building. It is never
stored, never serialised, never logged. `ServiceSpec` is therefore safe to persist, to render in the
GUI, to print in `mix service status --json`, and to attach to a bug report.

This generalises to a rule for the workspace: **a type that is authored by one layer and consumed by
another that cannot depend on it belongs in `proto`**, and is designed so it holds no secret. The
same shape is already latent in `PrivilegedOp`, which
[../architecture/platform-abstraction.md](../architecture/platform-abstraction.md) places in
`mixengine-platform` while `DaemonEvent::ElevationRequired { ops: Vec<PrivilegedOp> }` in proto
requires proto to know it — a dependency the layering forbids. Phase 4 resolves that the same way;
this ADR is the precedent, not the fix.

## Consequences

**Easy**: one definition of a service, so an extension manifest, a database row, a GUI form and a
supervised child all agree by construction rather than through conversion code. `ts-rs` gives the GUI
its types for free. The layering test needs no new edge, and `core` does not inherit the supervisor's
async and HTTP dependencies. Secret handling is enforced by the type rather than by review
discipline — there is no field a password fits into.

**Hard / accepted costs**:

- `proto` grows beyond the "request, response, and event types" its charter names. Accepted: the
  constraint that charter actually protects is **serde only, no I/O, no platform code**, which
  `ServiceSpec` satisfies. The charter wording is widened rather than the rule.
- `program` and `cwd` are absolute host paths inside a serialisable type. Accepted: `PodSpec` and the
  OCI runtime spec do the same, and a `ServiceSpec` is machine-local and regenerated from state,
  which the "generated config is disposable" rule in [../../CLAUDE.md](../../CLAUDE.md) already
  covers. The prohibition on absolute paths belongs to blueprints, a different artifact with a
  different lifetime.
- `EnvValue::Literal` can still be handed a password by a careless caller. Accepted: the affordance
  now points the other way and review has a name for the mistake, which is the most a type can do.
- Every spec-shaped thing must round-trip through serde, so an unrepresentable value — a closure, an
  open file descriptor — cannot enter a spec. This is a feature.

## Alternatives considered

- **`ServiceSpec` in `mixengine-supervisor`**, adding a `core → supervisor` edge. Matches the old
  sentence in `process-supervision.md` literally. Rejected: `ResourceLimits` and `IdlePolicy` must be
  in proto for `ts-rs` regardless, so the vocabulary splits anyway — and `core`, which already
  carries `sqlx`, would inherit the supervisor's tokio/regex/HTTP stack to describe an intention.
  The same trade was refused for `mixengine-cli` and is recorded in `workspace_layering.rs`.
- **`ServiceSpec` in `mixengine-core`**, adding a `supervisor → core` edge. Rejected outright: it
  puts a bundled SQLite inside the process supervisor.
- **Split by authorship**: policy types in proto, `ServiceSpec` in the supervisor without serde, the
  daemon composing the two. This is the Nomad model (`api` versus `nomad/structs` with conversion
  functions). Rejected for now: it pays for wire-versus-internal version decoupling, which is worth
  real money once an API is stable across years and independent clients, and worth nothing to a
  pre-1.0 workspace where both sides ship from the same commit. Revisit if `mixengine-proto` ever
  needs to support a client built against an older release. Note that this alternative does not
  solve the secret problem either — `EnvValue` is required in every variant.
- **A fourth crate below both**, owning only the spec. A crate whose whole content is ten structs,
  added to keep a sentence in a document true. Rejected as ceremony.
