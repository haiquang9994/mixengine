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

/// The arguments that list every inbound rule, in full — roadmap task **T76**.
///
/// Read-only and unprivileged, unlike everything else in this module: what it is for is finding the
/// rule *Windows* wrote for `mixengined.exe` when it asked about mDNS, which MixEngine never made
/// and therefore cannot know about from its own database.
#[must_use]
pub(crate) fn show() -> Vec<String> {
    vec![
        "advfirewall".to_owned(),
        "firewall".to_owned(),
        "show".to_owned(),
        "rule".to_owned(),
        "name=all".to_owned(),
        "dir=in".to_owned(),
        "verbose".to_owned(),
    ]
}

/// How many rule blocks in `listing` name `program` — roadmap task **T76**.
///
/// **The path is searched for, and nothing else is parsed. That is the decision, not a shortcut.**
/// `netsh` writes its field labels in the system's language: the line that reads `Program:` on an
/// English Windows reads something else on a Vietnamese one, so a parser keyed off those labels
/// reports zero rules on exactly the machines this check exists for. A path is a path in every
/// language.
///
/// Rules are separated by a blank line, which is the only structure relied on here.
#[must_use]
pub(crate) fn counted(listing: &str, program: &str) -> usize {
    // Windows does not preserve the case a path was written in, and `netsh` echoes back whatever
    // the rule holds rather than what was asked for.
    let program = program.to_lowercase();

    listing
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter(|block| block.to_lowercase().contains(&program))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The program path is searched for, and nothing else is parsed** — roadmap task **T76**, and
    /// the reason is measured rather than assumed: `netsh` localises its field labels, so a parser
    /// keyed off `Program:` reports nothing on a Windows that is not in English.
    #[test]
    fn a_rule_is_counted_when_its_block_names_the_program() {
        let listing = "\
Rule Name:                            mixengined
----------------------------------------------------------------------
Enabled:                              Yes
Profiles:                             Private,Public
Program:                              C:\\Users\\a\\bin\\mixengined.exe

Rule Name:                            Something else
----------------------------------------------------------------------
Enabled:                              Yes
Program:                              C:\\Windows\\System32\\svchost.exe
";

        assert_eq!(counted(listing, "C:\\Users\\a\\bin\\mixengined.exe"), 1);
        assert_eq!(counted(listing, "C:\\Windows\\System32\\svchost.exe"), 1);
        assert_eq!(counted(listing, "C:\\nothing\\here.exe"), 0);
    }

    /// Windows does not preserve the case a path was written in.
    #[test]
    fn the_comparison_ignores_case() {
        let listing = "Program:  C:\\Users\\A\\BIN\\MixEngineD.exe\n";

        assert_eq!(counted(listing, "c:\\users\\a\\bin\\mixengined.exe"), 1);
    }

    /// `netsh` writes CRLF, and a test written on any other system would not.
    #[test]
    fn blocks_are_separated_whichever_line_ending_the_tool_used() {
        let listing = "Program: C:\\a.exe\r\n\r\nProgram: C:\\a.exe\r\n";

        assert_eq!(counted(listing, "C:\\a.exe"), 2);
    }

    /// A machine with no such rule is the ordinary one, and it is zero rather than a failure.
    #[test]
    fn no_output_is_no_rules() {
        assert_eq!(counted("", "C:\\anything.exe"), 0);
    }

    #[test]
    fn the_listing_asks_for_inbound_rules_in_full() {
        let args = show();

        assert!(args.contains(&"dir=in".to_owned()), "{args:?}");
        assert!(args.contains(&"verbose".to_owned()), "{args:?}");
        assert!(args.contains(&"name=all".to_owned()), "{args:?}");
    }

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
