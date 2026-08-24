//! Whether a `keyring` failure means this machine has no credential store at all.
//!
//! **The one reading in this crate that is per-OS while its capability is not.** `crate::secrets`
//! is deliberately a single implementation, because the `keyring` crate is already the abstraction;
//! what is *not* shared is how each backend encodes "there is nothing here to talk to". Windows and
//! macOS say it in `keyring`'s own vocabulary. Linux does not say it at all, and this module is why
//! that costs a file.
//!
//! `keyring`'s secret-service backend maps `Locked`, `NoResult` and `Prompt` to `NoStorageAccess`
//! and **everything else** to `PlatformFailure` — so a session with no provider arrives as a
//! platform failure, and a keyring that is merely locked arrives as a machine with no store.
//! Both directions are wrong, and both are fixed here by ignoring `keyring`'s judgement on this
//! system and reading the D-Bus error *name* underneath it instead.
//!
//! **The name and not the message.** `dbus::Error`'s `Display` prints only the message, and the
//! message is the bus implementation's own wording: Ubuntu 24.04 answers an unreachable bus with
//! "Using X11 for dbus-daemon autolaunch was disabled at compile time" where the string everybody
//! quotes is "without a $DISPLAY for X11", and `dbus-broker` need not agree with `dbus-daemon`
//! about any of it. The names below are in the D-Bus specification, which is what makes them worth
//! matching. Reaching them is the whole reason this crate depends on `keyring`'s backend directly,
//! which `.claude/decisions/0013-reading-the-d-bus-error-name-to-tell-an-absent-store.md` argues.
//!
//! Every name here was measured rather than looked up, on a real machine, one environment each.

use keyring::error::Error as KeyringError;

/// The bus is there and nothing on it provides `org.freedesktop.secrets`.
///
/// A GitHub runner, a container, a desktop with the keyring package removed. This is the answer the
/// CI run that opened this question actually produced.
const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";

/// There is no session bus to reach and none can be started.
///
/// A plain `ssh` login on a server: no `DBUS_SESSION_BUS_ADDRESS`, no `XDG_RUNTIME_DIR`, and
/// autolaunch refused. **The case a headless machine actually hits** — it never reaches
/// [`SERVICE_UNKNOWN`], because it fails one step earlier, at the bus.
const NOT_SUPPORTED: &str = "org.freedesktop.DBus.Error.NotSupported";

/// `DBUS_SESSION_BUS_ADDRESS` names a socket that is not there.
///
/// A stale variable inherited by a `systemd` unit or a `cron` job, which is the shape a daemon meets
/// far more often than a person does.
const FILE_NOT_FOUND: &str = "org.freedesktop.DBus.Error.FileNotFound";

/// What a person can do about a machine with no secret service, told apart by which of the three it
/// is — because the three have nothing in common but the outcome.
fn workaround(name: &str) -> Option<&'static str> {
    match name {
        SERVICE_UNKNOWN => Some(
            "this session's D-Bus has no secret service on it — install and start one \
             (gnome-keyring, kwallet), or on a machine with no desktop run \
             `gnome-keyring-daemon --unlock --components=secrets` inside a session bus",
        ),
        NOT_SUPPORTED => Some(
            "this login has no D-Bus session bus at all and one cannot be started here — run under \
             `dbus-run-session`, or point DBUS_SESSION_BUS_ADDRESS at the bus of a logged-in \
             session",
        ),
        FILE_NOT_FOUND => Some(
            "DBUS_SESSION_BUS_ADDRESS names a socket that is not there — unset it to let this \
             machine find its own session bus, or point it at one that exists",
        ),
        // **Everything else is a store that is present and refusing**, which is the safe direction
        // to be wrong in: reporting a working machine as unsupported would send a person looking for
        // a keyring they already have. `AccessDenied` and `NoReply` both arrive here, and the second
        // is not hypothetical — a `gnome-keyring` that the bus activated and that then never
        // answered produces it.
        _ => None,
    }
}

/// The workaround for a machine with no credential store, or `None` when it has one.
///
/// `keyring`'s own `NoStorageAccess` is deliberately not consulted on this system: see the module
/// documentation for what it means here, which is the opposite of what it means on the other two.
pub(crate) fn absent_store(source: &KeyringError) -> Option<&'static str> {
    let KeyringError::PlatformFailure(inner) = source else {
        return None;
    };

    // The downcast is where two copies of `dbus-secret-service` in one tree would show up, and it
    // would show up as this returning `None` forever rather than as a build that fails — which is
    // why `lint` counts them. ADR 0013 records the check along with the decision it protects.
    let service = inner.downcast_ref::<dbus_secret_service::Error>()?;

    match service {
        // Declared for exactly this case and constructed nowhere in 4.1.0 — matched anyway, so the
        // day the backend starts answering properly this module needs no edit to agree.
        dbus_secret_service::Error::Unavailable => Some(
            "this machine has no secret service — install and start one (gnome-keyring, kwallet)",
        ),
        dbus_secret_service::Error::Dbus(bus) => workaround(bus.name()?),
        // The enum is `#[non_exhaustive]`, so this arm is required rather than chosen. It falls the
        // same way the unmatched names above do, and for the same reason.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three names a machine with no secret service answers with, each measured on a real one.
    #[test]
    fn the_three_shapes_of_having_no_store_each_carry_their_own_way_out() {
        for name in [SERVICE_UNKNOWN, NOT_SUPPORTED, FILE_NOT_FOUND] {
            let advice =
                workaround(name).unwrap_or_else(|| panic!("{name} means there is no store"));

            assert!(
                !advice.is_empty(),
                "{name} answered with an empty workaround, which rule 4 of \
                 platform-abstraction.md says is not an answer"
            );
        }

        assert_eq!(
            [SERVICE_UNKNOWN, NOT_SUPPORTED, FILE_NOT_FOUND]
                .map(|name| workaround(name).expect("measured"))
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3,
            "two of the three got the same advice, and they have nothing in common but the outcome"
        );
    }

    /// A store that is there and refusing is not a machine that has none.
    #[test]
    fn a_store_that_answers_and_refuses_is_never_read_as_an_absent_one() {
        for name in [
            // The bus refusing our call.
            "org.freedesktop.DBus.Error.AccessDenied",
            // Measured: a `gnome-keyring` the bus activated and that then never answered.
            "org.freedesktop.DBus.Error.NoReply",
            // The secret service's own vocabulary — a locked collection, which `keyring` reports as
            // `NoStorageAccess` and which this system must not read as an absent store.
            "org.freedesktop.Secret.Error.IsLocked",
            "",
        ] {
            assert_eq!(workaround(name), None, "{name} was read as an absent store");
        }
    }

    /// `keyring`'s own "no storage access" means a locked keyring on this system, not a missing one.
    #[test]
    fn keyrings_no_storage_access_is_not_this_systems_answer_for_an_absent_store() {
        let locked = KeyringError::NoStorageAccess(Box::new(std::io::Error::other(
            "Secret Service: object locked",
        )));

        assert_eq!(absent_store(&locked), None);
    }

    /// A platform failure from something that is not the secret service at all.
    #[test]
    fn a_failure_this_module_cannot_read_leaves_the_verdict_alone() {
        let opaque =
            KeyringError::PlatformFailure(Box::new(std::io::Error::other("something else")));

        assert_eq!(absent_store(&opaque), None);
        assert_eq!(absent_store(&KeyringError::NoEntry), None);
    }
}
