//! Linux' two `systemd-networkd` files, as text.
//!
//! **A link of our own is the only mechanism that works**, and every alternative the feature spec
//! and the roadmap named was measured out — the T45 design, D10:
//!
//! - a `resolved.conf.d` drop-in with a global routing domain redirects the **whole machine**:
//!   after it, `getent hosts github.com` answered `127.0.0.1`;
//! - `resolvectl dns lo …` is refused by name — *"Link lo is loopback device"*;
//! - a real link would have its own servers **replaced** rather than added to;
//! - and a link with no address is accepted, reports the servers back, and never gets a DNS scope,
//!   so no query is ever sent. That is the worst failure shape of the four, because it reads as
//!   applied.
//!
//! Pure and compiled everywhere, for [`crate::resolver::directory`]'s reason.

/// The interface MixEngine creates.
pub(crate) const LINK: &str = "mixengine0";

/// What makes the link `routable`, which is the only property being bought.
///
/// **`/32`, so it adds no route anything else can reach** — and deliberately *not* the link-local
/// address an earlier draft of this file carried. Two measurements were run: one that brought the
/// link up with `ip addr add`, where a link-local address was enough to give it a DNS scope, and one
/// that declared the link in these files, where the address was `10.53.53.53/32`. Only the second is
/// the mechanism this ships, and taking a fact from the first into the shape of the second is
/// exactly the remembering the design's measurements exist to replace. CI caught it: the wiring
/// applied, the probe agreed, and no name resolved.
const ADDRESS: &str = "10.53.53.53/32";

/// Where the file that declares the link goes.
pub(crate) const NETDEV_PATH: &str = "/etc/systemd/network/10-mixengine.netdev";

/// Where the file that configures it goes.
pub(crate) const NETWORK_PATH: &str = "/etc/systemd/network/10-mixengine.network";

/// The header both files carry, so somebody who finds one knows what wrote it and that editing it
/// is pointless.
const HEADER: &str = "# Managed by MixEngine. Regenerated from this home's state; do not edit.";

/// The file that declares the link.
pub(crate) fn netdev() -> String {
    format!("{HEADER}\n[NetDev]\nName={LINK}\nKind=dummy\n")
}

/// The file that gives it an address, a DNS server and its routing domains.
pub(crate) fn network(tlds: &[String], port: u16) -> String {
    let domains = tlds
        .iter()
        .map(|tld| format!("~{tld}"))
        .collect::<Vec<_>>()
        .join(" ");

    format!(
        "{HEADER}\n[Match]\nName={LINK}\n\n[Network]\nAddress={ADDRESS}\n\
         DNS=127.0.0.1:{port}\nDomains={domains}\nLinkLocalAddressing=no\nIPv6AcceptRA=no\n"
    )
}

/// The routing domains a `.network` file declares, and the port its DNS server is on.
///
/// Reading the file back rather than remembering what was written is what lets the daemon's probe
/// answer "is this still wired?" after an update, an edit or another home's write.
pub(crate) fn declared(contents: &str) -> (Vec<String>, Option<u16>) {
    let mut domains = Vec::new();
    let mut port = None;

    for line in contents.lines() {
        let line = line.trim();

        if let Some(value) = line.strip_prefix("Domains=") {
            domains = value
                .split_whitespace()
                .filter_map(|domain| domain.strip_prefix('~'))
                .map(str::to_owned)
                .collect();
        } else if let Some(value) = line.strip_prefix("DNS=") {
            port = value
                .strip_prefix("127.0.0.1:")
                .and_then(|number| number.parse().ok());
        }
    }

    (domains, port)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The link has to carry an address or systemd-resolved builds no DNS scope for it and the
    /// whole thing is configured and inert — measured, and the reason round two of the probe
    /// looked like it had worked when it had not.
    #[test]
    fn the_network_file_gives_the_link_an_address() {
        let file = network(&["test".to_owned()], 53_535);

        assert!(file.contains("Address=10.53.53.53/32"), "{file}");
        assert!(file.contains("DNS=127.0.0.1:53535"), "{file}");
        assert!(file.contains("[Match]\nName=mixengine0"), "{file}");
    }

    /// Every TLD is a **routing** domain. The tilde is what keeps the link from answering for
    /// anything else, and losing it is exactly how this becomes a machine-wide DNS change.
    #[test]
    fn every_tld_is_a_routing_domain_and_none_is_a_search_domain() {
        let file = network(&["test".to_owned(), "internal".to_owned()], 53_535);

        let domains = file
            .lines()
            .find(|line| line.starts_with("Domains="))
            .expect("a Domains= line");

        assert_eq!(domains, "Domains=~test ~internal");
    }

    /// Both files go where `systemd-networkd` reads administrator configuration from, and both are
    /// prefixed so they sort before a distribution's own. Measured paths, so they are asserted
    /// rather than trusted to stay right through a later edit.
    #[test]
    fn both_files_live_where_networkd_reads_them() {
        for path in [NETDEV_PATH, NETWORK_PATH] {
            assert!(
                path.starts_with("/etc/systemd/network/"),
                "{path} is not where networkd looks"
            );
            assert!(path.contains("mixengine"), "{path} does not name us");
        }

        assert!(NETDEV_PATH.ends_with(".netdev"));
        assert!(NETWORK_PATH.ends_with(".network"));
    }

    #[test]
    fn the_netdev_declares_a_dummy_link_by_our_name() {
        let file = netdev();

        assert!(file.contains("Kind=dummy"), "{file}");
        assert!(file.contains(&format!("Name={LINK}")), "{file}");
    }

    /// What is written is what is read back, which is the whole of the probe's comparison.
    #[test]
    fn a_file_reads_back_as_what_it_declared() {
        let file = network(&["test".to_owned(), "localhost".to_owned()], 53_535);

        let (domains, port) = declared(&file);

        assert_eq!(domains, vec!["test".to_owned(), "localhost".to_owned()]);
        assert_eq!(port, Some(53_535));
    }

    /// A file naming a server that is not this home's loopback server declares no port we can use,
    /// so the probe reports nothing wired rather than claiming somebody else's link.
    #[test]
    fn a_file_naming_another_server_declares_no_port() {
        let (_domains, port) = declared("DNS=10.0.0.1:53\nDomains=~test\n");

        assert_eq!(port, None);
    }

    /// An empty or unrelated file is not a wiring, and is not an error either.
    #[test]
    fn a_file_that_declares_nothing_reads_back_as_nothing() {
        let (domains, port) = declared("");

        assert!(domains.is_empty());
        assert_eq!(port, None);
    }
}
