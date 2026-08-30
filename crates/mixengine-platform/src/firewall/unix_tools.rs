//! The two Linux firewalls MixEngine will drive, and the sentence for a machine running neither.

/// Which firewall a Linux machine is running, if either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// `ufw`, and it is active.
    Ufw,

    /// `firewalld`, and it is running.
    Firewalld,
}

/// `ufw allow <port>/tcp`, one invocation per port.
///
/// **`ufw` has no comment field on a plain allow**, so unlike `netsh` the label cannot be written
/// into the rule. What that costs is the ability to enumerate MixEngine's own rules later, which is
/// why `Applied::Written` names the ports it opened rather than promising they can be found by
/// name — and why T76's "no rule left behind" test is a Windows test.
#[must_use]
pub(crate) fn ufw(port: u16, allow: bool) -> Vec<String> {
    let verb = if allow { "allow" } else { "deny" };

    vec![verb.to_owned(), format!("{port}/tcp")]
}

/// `firewall-cmd --add-port=<port>/tcp`, runtime only.
///
/// **Not `--permanent`.** A shared site is a thing you turn on for an afternoon; a rule that
/// survived a reboot would outlive both the share and the reason for it, and T76 revokes on a
/// network change rather than on a restart.
#[must_use]
pub(crate) fn firewalld(port: u16, allow: bool) -> Vec<String> {
    let verb = if allow { "add" } else { "remove" };

    vec![format!("--{verb}-port={port}/tcp")]
}

/// What to tell a user whose machine MixEngine cannot configure, and what they would run instead.
///
/// The command is `ufw`'s, because a machine with neither firewall running is a machine where
/// nothing is blocking the port in the first place — the sentence is what matters, and the command
/// is there for the case where something else on the machine turns out to be filtering.
#[must_use]
pub(crate) fn unmanaged(ports: &[u16]) -> (String, String) {
    let reason = "no firewall this build can drive is running here — neither ufw nor firewalld — \
                  so nothing was changed; if the site is not reachable, something else on this \
                  machine is filtering the port"
        .to_owned();

    let manual = ports
        .iter()
        .map(|port| format!("sudo ufw allow {port}/tcp"))
        .collect::<Vec<_>>()
        .join(" && ");

    (reason, manual)
}

/// macOS, where a listening socket needs no rule at all.
#[must_use]
pub(crate) fn macos_unmanaged() -> (String, String) {
    (
        "macOS' application firewall filters applications rather than ports, and in its default \
         configuration a listening socket needs no rule — so nothing was changed"
            .to_owned(),
        // Deliberately empty: telling a user to add the front end to the application firewall's
        // allow list would be advice for a machine whose firewall is set to block all incoming
        // connections, and on that machine it is the front end's binary and not a port that has to
        // be named — which changes with every runtime version and is not a command worth printing.
        String::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ufw_allows_and_denies_the_same_port_by_the_same_spelling() {
        assert_eq!(ufw(8080, true), vec!["allow", "8080/tcp"]);
        assert_eq!(ufw(8080, false), vec!["deny", "8080/tcp"]);
    }

    /// A rule that outlived the reboot would outlive the reason for it.
    #[test]
    fn firewalld_is_never_asked_for_a_permanent_rule() {
        assert_eq!(firewalld(443, true), vec!["--add-port=443/tcp"]);
        assert_eq!(firewalld(443, false), vec!["--remove-port=443/tcp"]);
    }

    #[test]
    fn an_unmanaged_linux_machine_gets_a_command_per_port() {
        let (reason, manual) = unmanaged(&[80, 443]);

        assert!(
            reason.contains("ufw") && reason.contains("firewalld"),
            "{reason}"
        );
        assert_eq!(manual, "sudo ufw allow 80/tcp && sudo ufw allow 443/tcp");
    }

    #[test]
    fn macos_says_why_and_suggests_nothing() {
        let (reason, manual) = macos_unmanaged();

        assert!(reason.contains("application firewall"), "{reason}");
        assert!(manual.is_empty(), "{manual}");
    }
}
