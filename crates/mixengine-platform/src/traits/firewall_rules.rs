//! Firewall rules this machine holds for a program — roadmap task **T76**.

use std::path::Path;

use crate::Result;

/// What inbound firewall rules name a given program.
///
/// **Reads only, and needs no privilege.** That is what separates this from [`crate::firewall`],
/// which *writes*: that module lives behind the `elevated` feature and only `mixengine-elevate`
/// ever calls it, because opening a port needs a token the daemon does not have. Reading the rule
/// list needs nothing, changes nothing, and answers a question the daemon's own database cannot —
/// see [`naming`](Self::naming).
pub trait FirewallRules: std::fmt::Debug + Send + Sync {
    /// How many inbound rules name `program`, or [`None`] where this system has no such mechanism.
    ///
    /// **The question is about a rule MixEngine did not make.** Binding UDP 5353 for mDNS makes
    /// Windows raise its own firewall dialog, and Allow writes an every-port TCP-and-UDP rule for
    /// `mixengined.exe` on the Private *and* Public profiles. It is far wider than the web ports a
    /// shared site needs, it was not created through `mixengine-elevate`, and `site.unshare` does
    /// not remove it — because MixEngine never made it and does not delete what it did not make.
    /// What this build does about it is say that it is there.
    ///
    /// **A count and not a list.** What is wanted is *does that rule exist?*, and the answer is
    /// rendered as a sentence with a command beside it. Parsing rule names out of a firewall tool
    /// would mean parsing localised field labels; a count needs only the program's path, which is
    /// the same string in every language.
    ///
    /// MixEngine's own rules are scoped by port and carry no program, so nothing here can count one
    /// of ours by mistake.
    ///
    /// [`None`] on macOS and Linux: neither has a per-program inbound rule table this build reads.
    ///
    /// # Errors
    ///
    /// [`Error::Os`](crate::Error::Os) where the tool could not be run at all.
    fn naming(&self, program: &Path) -> Result<Option<usize>>;
}
