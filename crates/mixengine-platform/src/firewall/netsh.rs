//! The arguments Windows' firewall is driven with, built here so they can be tested anywhere.

/// The arguments that delete every rule under `label`.
///
/// **Delete before add, always** — that is what makes a whole-state plan idempotent on a tool whose
/// `add rule` appends rather than replaces. Deleting a name that is not there is not an error worth
/// reporting: `netsh` says "No rules match the specified criteria" and exits non-zero, and the
/// caller reads that as nothing-to-do rather than as a failure.
#[must_use]
pub(crate) fn delete(label: &str) -> Vec<String> {
    vec![
        "advfirewall".to_owned(),
        "firewall".to_owned(),
        "delete".to_owned(),
        "rule".to_owned(),
        format!("name={label}"),
    ]
}

/// The arguments that open `ports` inbound over TCP under `label`.
///
/// One rule for every port at once: `localport` takes a list, and one rule is one thing to find,
/// one thing to show a person, and one thing to delete.
#[must_use]
pub(crate) fn add(label: &str, ports: &[u16]) -> Vec<String> {
    let list = ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");

    vec![
        "advfirewall".to_owned(),
        "firewall".to_owned(),
        "add".to_owned(),
        "rule".to_owned(),
        format!("name={label}"),
        "dir=in".to_owned(),
        "action=allow".to_owned(),
        "protocol=TCP".to_owned(),
        format!("localport={list}"),
        // The private profile only. A shared site is for the network the machine is already trusted
        // on; a rule that also applied to `public` would follow the laptop to a café, which is
        // exactly what T76's auto-revoke exists to prevent and would be pointless to leave open
        // here in the meantime.
        "profile=private".to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deletion_names_the_label_and_nothing_else() {
        assert_eq!(
            delete("MixEngine — blog"),
            vec![
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                "name=MixEngine — blog"
            ]
        );
    }

    #[test]
    fn one_rule_carries_every_port() {
        let args = add("MixEngine — blog", &[80, 443]);

        assert!(args.contains(&"localport=80,443".to_owned()), "{args:?}");
        assert_eq!(
            args.iter().filter(|arg| arg.starts_with("name=")).count(),
            1,
            "{args:?}"
        );
    }

    /// A rule that applied to the public profile would follow the laptop onto a café network.
    #[test]
    fn the_rule_is_private_profile_only() {
        assert!(add("MixEngine — blog", &[80]).contains(&"profile=private".to_owned()));
    }

    #[test]
    fn the_rule_is_inbound_tcp_and_says_so_explicitly() {
        let args = add("MixEngine — blog", &[8080]);

        assert!(args.contains(&"dir=in".to_owned()), "{args:?}");
        assert!(args.contains(&"protocol=TCP".to_owned()), "{args:?}");
        assert!(args.contains(&"action=allow".to_owned()), "{args:?}");
    }
}
