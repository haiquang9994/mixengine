# 0013. Reading the D-Bus error name to tell an absent credential store from a refusing one

**Status**: Accepted
**Date**: 2026-08-24
**Qualifies** the one-crate-per-concern table in
[../standards/rust.md](../standards/rust.md), which otherwise stands unchanged, and is what makes
rule 4 of [../architecture/platform-abstraction.md](../architecture/platform-abstraction.md) true on
Linux.

## Context

Rule 4 says a capability the machine does not have is a normal answer carrying a workaround, and
never a failure of ours. `mixengine-platform`'s credential store could not obey it on Linux, because
the crate it is built on cannot tell the two apart.

`keyring`'s secret-service backend classifies in four lines:

```rust
match err {
    Error::Locked   => no_access(err),
    Error::NoResult => no_access(err),
    Error::Prompt   => no_access(err),
    _               => platform_failure(err),
}
```

`no_access` is `NoStorageAccess`, which on Windows and macOS means exactly "there is no store here"
— `ERROR_NO_SUCH_LOGON_SESSION`, `errSecNoSuchKeychain` and its three siblings. On Linux it means a
keyring that is **present and locked**, and a session with no secret service at all falls through to
`PlatformFailure`. So the same enum answers the question correctly on two systems and backwards in
both directions on the third: a headless machine is told MixEngine failed, and a locked keyring is
told the machine has no store.

**Three things were ruled out before this one.**

*Wait for the crate to fix it.* `keyring` 4.1.6 restructures into `keyring-core` plus per-store
crates and changes nothing here: `dbus-secret-service-keyring-store` 1.0.0 carries the same four
lines, `Service::new` maps a failed connect to `PlatformFailure`, and `keyring-core::Error` has no
variant for an absent store either. Upgrading is a migration, not a fix.

*Read the variant the backend already has.* `dbus_secret_service::Error::Unavailable` is documented
as "a secret service provider, or a session to connect to one, was not found on the system" — and in
4.1.0 it is **constructed nowhere**. It exists in the enum and in the `Display` arm, and no code path
produces it.

*Match the message text.* No new dependency, and wrong today rather than wrong eventually.
`dbus::Error`'s `Display` prints `message()` and never `name()`, so matching text means matching a
bus implementation's own wording — and two implementations are already deployed, `dbus-daemon` and
`dbus-broker`, which are not obliged to agree. The wording moves inside one of them too: a plain
`ssh` login on Ubuntu 24.04 answers "Using X11 for dbus-daemon autolaunch was disabled at compile
time, set your DBUS_SESSION_BUS_ADDRESS instead", not the "without a $DISPLAY for X11" that every
account of this failure quotes, because that distribution built dbus with X11 autolaunch off.

## Decision

**`mixengine-platform` depends on `dbus-secret-service` directly on Linux, downcasts `keyring`'s
boxed source to it, and reads the D-Bus error name.** The name is the field the D-Bus specification
fixes, so every implementation of the bus must use the same one.

The list of names meaning *this machine has no secret service* is closed, and each was measured on a
real Linux with a probe built against these exact crate versions rather than looked up:

| What the machine is | D-Bus error name |
| --- | --- |
| A bus is there and nothing provides `org.freedesktop.secrets` | `org.freedesktop.DBus.Error.ServiceUnknown` |
| No session bus to reach, and none that can be started | `org.freedesktop.DBus.Error.NotSupported` |
| `DBUS_SESSION_BUS_ADDRESS` names a socket that is not there | `org.freedesktop.DBus.Error.FileNotFound` |

`Unavailable` is matched as well, so the day the backend starts constructing it nothing here needs
editing to agree.

**`keyring`'s own `NoStorageAccess` is not consulted on Linux at all**, and is the whole answer on
the other two. That is why the reading lives in three small `sys::secrets` modules rather than in
the one `Secrets` implementation above them: what is per-OS is not the capability, which the crate
already abstracts, but how each backend spells "there is nothing here".

**Anything not on the list stays `Error::Secret`.** Being wrong in that direction reports a working
machine's store as having refused, which sends a person to look at a keyring they have; being wrong
the other way sends them looking for one they already own. `AccessDenied` and `NoReply` both land
there, and the second is not hypothetical — measuring turned one up, a `gnome-keyring` the bus
activated that then never answered.

## Consequences

**This pins `mixengine-platform` to `keyring`'s current choice of Linux backend**, which is the cost
the one-crate-per-concern table exists to avoid and the reason this is an ADR. `keyring` has changed
that backend before and 4.x offers a `zbus` store beside the `dbus` one. If it moves, this module
moves with it — the fix is to follow, never to widen the match into text.

**The failure mode is silence, so it is counted rather than trusted.** Two versions of
`dbus-secret-service` in one tree — ours and the one `keyring` resolves — is not a build failure and
not a test failure. The downcast simply answers `None` for ever, and every machine without a keyring
goes back to being told its store refused. `lint` asks the tree how many there are, and one is the
only right answer.

**The helper is unaffected.** The dependency is optional and enabled by `host`; `mixengine-elevate`
compiles this crate with `elevated` alone, and its closure has no D-Bus in it. That is checked by the
step already guarding its dependency budget.

**A skip is not a proof, so CI takes the store away on purpose.** Every other leg hands the tests a
working store, which leaves the absent branch a path no green run walks — which is how this bug
survived to be found by a stack trace rather than by the suite built to find it.
`test-absent-secret-service.sh` runs the credential tests once per name above, with
`MIXENGINE_TEST_NO_KEYRING=1` so that a round whose sabotage did not work fails instead of passing
quietly.

**A machine with no store still cannot run a service that needs a credential**, and that is
unchanged: `services/first_run.rs` refuses rather than writing a password to disk, because
[0006](0006-servicespec-in-proto-and-secret-free.md) gives a root password exactly one home. What
changes is that the refusal now names what is missing and what to do about it, per name, instead of
reporting MixEngine as broken.
