//! Why a start that failed failed, when the answer is a port — roadmap task **T38**.
//!
//! Every service in this product binds something, and the likeliest reason one of them will not
//! start on a developer's machine is that a program MixEngine does not manage is already on its
//! port: an XAMPP, a Homebrew MariaDB, Windows' own `MySQL80` service. None of those has a
//! `services` row, so the daemon cannot answer this from its own state and asks the OS.
//!
//! **This runs after the failure, never before the start.** A check beforehand would be a race —
//! a port free when it was asked about and taken a moment later — and would put an OS call in front
//! of every start for the sake of the rare one that fails. What is here instead is a diagnosis: the
//! start already failed, the reason is about to be written down, and this decides whether there is
//! a better sentence available than the symptom.

use mixengine_platform::Host;
use mixengine_proto::StateReason;

/// The reason a failed start deserves, when one of `ports` is held by somebody else.
///
/// `ours` is the pid of the process this start ran, where there is one. It is what separates the
/// two cases that look identical in the listening table: a service that came up, bound its port and
/// then failed its ready check is holding that port *itself*, and reporting it as a conflict would
/// send a user hunting for a program that is MixEngine's own.
///
/// **Every failure to ask is [`None`].** The caller is on an error path with a reason already in
/// hand, and a diagnosis that cannot be made must leave that reason alone rather than replace it
/// with one about the diagnosis — see [`mixengine_platform::PortOwner`].
pub(super) fn conflict(host: &dyn Host, ports: &[u16], ours: Option<u32>) -> Option<StateReason> {
    ports.iter().find_map(|&port| {
        let holder = host.port_owner().listening_on(port).ok()??;

        if holder.pid.is_some() && holder.pid == ours {
            return None;
        }

        Some(StateReason::PortInUse {
            port,
            pid: holder.pid,
            program: holder.name,
        })
    })
}

#[cfg(test)]
mod tests {
    use mixengine_platform::{PortHolder, mock};

    use super::*;

    /// The home every mock here is given. Nothing in this module touches it.
    const HOME: &str = "/mixengine";

    /// The pid of the process a start ran, for the tests that need one.
    const OURS: u32 = 999;

    fn squatter() -> PortHolder {
        PortHolder {
            pid: Some(4242),
            name: Some("mysqld.exe".to_owned()),
        }
    }

    #[test]
    fn a_declared_port_held_by_another_process_is_the_reason() {
        let host = mock::Host::with_a_port_held(HOME, 3306, squatter());

        assert_eq!(
            conflict(&host, &[3306], Some(OURS)),
            Some(StateReason::PortInUse {
                port: 3306,
                pid: Some(4242),
                program: Some("mysqld.exe".to_owned()),
            })
        );
    }

    /// The ready check failed and the service itself is on the port: not a conflict.
    #[test]
    fn a_port_this_service_is_holding_itself_is_not_a_conflict() {
        let host = mock::Host::with_a_port_held(
            HOME,
            3306,
            PortHolder {
                pid: Some(OURS),
                name: Some("mariadbd".to_owned()),
            },
        );

        assert_eq!(conflict(&host, &[3306], Some(OURS)), None);
    }

    /// A holder nobody can identify is still a holder, and is still the better sentence.
    #[test]
    fn a_port_held_by_somebody_who_cannot_be_named_is_still_the_reason() {
        let host = mock::Host::with_a_port_held(
            HOME,
            3306,
            PortHolder {
                pid: None,
                name: None,
            },
        );

        assert_eq!(
            conflict(&host, &[3306], Some(OURS)),
            Some(StateReason::PortInUse {
                port: 3306,
                pid: None,
                program: None,
            })
        );
    }

    #[test]
    fn a_port_nobody_is_listening_on_is_not_a_conflict() {
        let host = mock::Host::with_home(HOME);

        assert_eq!(conflict(&host, &[3306], Some(OURS)), None);
    }

    #[test]
    fn a_service_that_declares_no_ports_has_nothing_to_diagnose() {
        let host = mock::Host::with_a_port_held(HOME, 3306, squatter());

        assert_eq!(conflict(&host, &[], Some(OURS)), None);
    }

    /// The rule the whole module is written around, stated as a test so it cannot quietly change.
    #[test]
    fn a_machine_that_cannot_be_asked_leaves_the_failure_as_it_was() {
        let host = mock::Host::unable_to_name_ports(HOME, "no listening table here");

        assert_eq!(conflict(&host, &[3306], Some(OURS)), None);
    }

    /// The first port that is taken wins, and the ones after it are not asked about.
    #[test]
    fn the_first_held_port_is_the_one_reported() {
        let host = mock::Host::with_a_port_held(HOME, 3306, squatter());

        assert_eq!(
            conflict(&host, &[8080, 3306], Some(OURS)),
            Some(StateReason::PortInUse {
                port: 3306,
                pid: Some(4242),
                program: Some("mysqld.exe".to_owned()),
            })
        );
    }
}
