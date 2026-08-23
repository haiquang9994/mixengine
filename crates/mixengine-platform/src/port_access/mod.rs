//! Being allowed to answer on a port the operating system reserves — roadmap task **T42**.
//!
//! **Not [`crate::PortOwner`]**, which is about who got to a port first. This is about whether an
//! unprivileged program may bind one at all, and the three systems answer it three different ways:
//! Windows reserves nothing, Linux puts a capability on the binary, macOS redirects the packet
//! through its packet filter. See the T42 design, D2.
//!
//! Compiled under **both** `host` and `elevated`, for `crate::hosts`' reason: the daemon reads the
//! state and the helper writes it, and neither is worth a second implementation.

pub(crate) mod capability;
