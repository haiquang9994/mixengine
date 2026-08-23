//! Windows' Name Resolution Policy rule, as registry values.
//!
//! **Written directly rather than through `Add-DnsClientNrptRule`** — the T45 design, D11.
//! `mixengine-elevate` never runs arbitrary commands, and a fixed cmdlet with validated arguments
//! is still a scripting host started by a process holding an administrative token. The measurement
//! is what makes the alternative available: a rule written to exactly these values **is read back
//! by `Get-DnsClientNrptRule`**, so what MixEngine writes is what Windows' own tooling sees, and
//! what a user can remove without MixEngine's help.
//!
//! Pure and compiled everywhere, for [`crate::resolver::directory`]'s reason.

/// The one rule MixEngine owns.
///
/// **Fixed rather than generated**, which is what makes whole state cheap: "already done" is a read
/// of one key, unwiring is a delete of one key, and two homes on one machine converge on one rule
/// rather than accumulating a rule each.
pub(crate) const GUID: &str = "{6D1F0B2E-3A4C-4E5A-9C7D-1B8E5F2A6C43}";

/// Where the DNS Client keeps local policy rules, relative to `HKEY_LOCAL_MACHINE`.
pub(crate) const KEY: &str = concat!(
    r"SYSTEM\CurrentControlSet\services\Dnscache\Parameters\DnsPolicyConfig\",
    "{6D1F0B2E-3A4C-4E5A-9C7D-1B8E5F2A6C43}"
);

/// The address every rule MixEngine writes names.
///
/// A constant here and not a parameter, for the same reason the operation carries none — D3.
pub(crate) const SERVER: &str = "127.0.0.1";

/// The values one rule is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NrptValues {
    /// `Name`, a `REG_MULTI_SZ` of suffixes, each with the leading dot NRPT spells them with.
    pub(crate) names: Vec<String>,

    /// `GenericDNSServers`, a `REG_SZ`. Compiled in, never taken from a request — D3.
    pub(crate) servers: String,

    /// `ConfigOptions`, a `REG_DWORD`. `8` is what the cmdlet writes for a rule carrying DNS
    /// servers, measured off a rule it wrote.
    pub(crate) config_options: u32,

    /// `Version`, a `REG_DWORD`.
    pub(crate) version: u32,
}

/// The rule that routes every TLD in `tlds`.
pub(crate) fn values(tlds: &[String]) -> NrptValues {
    NrptValues {
        names: tlds.iter().map(|tld| format!(".{tld}")).collect(),
        servers: SERVER.to_owned(),
        config_options: 8,
        version: 2,
    }
}

/// Which of `want` a rule already routes here, given whatever is on the machine.
///
/// [`None`] is a machine that has never run MixEngine, which is not a failure. A rule naming
/// somebody else's server is not ours to count, however familiar its namespaces look.
pub(crate) fn wired_from(present: Option<&NrptValues>, want: &[&str]) -> Vec<String> {
    let Some(present) = present else {
        return Vec::new();
    };

    if present.servers != SERVER {
        return Vec::new();
    }

    want.iter()
        .filter(|tld| present.names.iter().any(|name| name == &format!(".{tld}")))
        .map(|tld| (*tld).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each namespace is a suffix rule, which NRPT spells with a leading dot.
    #[test]
    fn every_namespace_is_a_suffix_rule() {
        let values = values(&["test".to_owned(), "internal".to_owned()]);

        assert_eq!(
            values.names,
            vec![".test".to_owned(), ".internal".to_owned()]
        );
    }

    /// The four values measured off a rule the cmdlet wrote, so that what MixEngine writes is what
    /// `Get-DnsClientNrptRule` reads back.
    #[test]
    fn the_values_are_the_ones_measured_off_a_real_rule() {
        let values = values(&["test".to_owned()]);

        assert_eq!(values.servers, "127.0.0.1");
        assert_eq!(values.config_options, 8);
        assert_eq!(values.version, 2);
    }

    /// One key, not one per TLD — which is what makes "already done" a single read.
    #[test]
    fn the_key_is_one_fixed_guid_under_the_dns_client() {
        assert!(KEY.ends_with(GUID), "{KEY}");
        assert!(KEY.contains("DnsPolicyConfig"), "{KEY}");
    }

    #[test]
    fn a_rule_holding_our_names_reports_them_wired() {
        let present = values(&["test".to_owned(), "localhost".to_owned()]);

        assert_eq!(
            wired_from(Some(&present), &["test", "localhost"]),
            vec!["test".to_owned(), "localhost".to_owned()]
        );
    }

    /// A rule naming our TLDs but somebody else's server is not ours to count.
    #[test]
    fn a_rule_naming_another_server_is_not_wired() {
        let mut present = values(&["test".to_owned()]);
        present.servers = "10.0.0.1".to_owned();

        assert!(wired_from(Some(&present), &["test"]).is_empty());
    }

    /// A rule that routes less than was asked reports what it does route, not what it should.
    #[test]
    fn a_rule_missing_a_namespace_reports_only_what_it_has() {
        let present = values(&["test".to_owned()]);

        assert_eq!(
            wired_from(Some(&present), &["test", "internal"]),
            vec!["test".to_owned()]
        );
    }

    /// No rule at all is the ordinary state of a machine that has never run MixEngine.
    #[test]
    fn no_rule_is_nothing_wired() {
        assert!(wired_from(None, &["test"]).is_empty());
    }
}
