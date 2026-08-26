//! How busy a port is, as opposed to who is holding it.

use crate::Result;

/// How many TCP connections are established to a local port.
///
/// **Beside [`PortOwner`](crate::PortOwner) rather than inside it**, and the two are near-opposites.
/// `listening_on` answers *who is in my way* and builds a [`PortHolder`](crate::PortHolder) with a
/// pid and a program name in it, which costs a walk of `/proc` on Linux and a second process on
/// macOS. This is asked every sweep, for every service with an idle policy, for as long as the
/// daemon runs, and what it wants is a number.
///
/// **A count and not a list.** Which peer is connected is a question nothing here asks, and every
/// shape that could answer it is an allocation taken each tick to be dropped. If a client ever wants
/// *who is using this database*, that is a diagnostic and can have `listening_on`'s treatment then.
pub trait ConnectionCount: std::fmt::Debug + Send + Sync {
    /// How many TCP connections are established to `port` on this machine.
    ///
    /// Loopback and every-interface sockets both count, and a socket that is merely *listening* does
    /// not — this is exactly the half
    /// [`PortOwner::listening_on`](crate::PortOwner::listening_on) excludes, and a running service
    /// shows up in both readings at once.
    ///
    /// # Errors
    ///
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) where this OS offers no way
    /// to ask, [`Error::Command`](crate::Error::Command) where the tool it asks through failed, and
    /// [`Error::Io`](crate::Error::Io) for the read itself.
    ///
    /// **An error is not a count of zero, and no caller may treat it as one.** This is
    /// `PortOwner`'s rule with the stakes raised: there, a failed reading costs a diagnosis; here,
    /// reading *I could not measure* as *there is nothing to measure* stops a database somebody is
    /// using because a tool was missing. The idle sweeper resets its count on an error rather than
    /// advancing it, so an unmeasurable service runs forever instead of being stopped wrongly.
    fn established_on(&self, port: u16) -> Result<usize>;
}
