//! Being *allowed* to bind a port the operating system reserves.

use std::path::Path;

use mixengine_proto::privileged::{PortAccessPlan, PortAccessTarget, PortRedirect};

use crate::Result;

/// The first port an ordinary account may bind on either Unix.
const FIRST_UNRESERVED: u16 = 1024;

/// How this machine lets a program the user runs answer on a port the OS reserves.
///
/// **One per system, chosen by the system, never negotiated** — the T42 design, D2. There is no
/// fallback chain, because the measurement removed the only candidate for one: `reservedhigh` does
/// not exist on macOS 15, so that system has pf or it has nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortAccessMethod {
    /// Nothing is needed, and nothing is granted. Windows reserves no ports below 1024.
    Direct,

    /// A capability on the binary, which then binds the reserved port itself. Linux.
    Capability,

    /// A packet-filter redirect; the program binds an ordinary port instead. macOS.
    Redirect,
}

/// One port a site is reached on, and the port a program must bind to answer it.
///
/// **This is the value that keeps `#[cfg]` out of `mixengine-core`.** On macOS a front end binds
/// 8080 to answer on 80; on the other two it binds 80. The configuration generator needs to know
/// which, and may not ask what system it is on — so it asks here, exactly as it asks
/// [`PortOwner`](crate::PortOwner) who holds a port rather than reading a table itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortBinding {
    /// What a browser asks for.
    pub answer: u16,

    /// What a program must actually listen on.
    pub bind: u16,
}

/// What this machine needs before the ports asked about can be served, and whether it is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortAccessState {
    /// Which mechanism this system uses.
    pub method: PortAccessMethod,

    /// One entry per port that was asked about, in the order they were asked.
    pub bindings: Vec<PortBinding>,

    /// Is the grant already in place — or was none ever needed?
    pub granted: bool,

    /// Why not, in words, when `granted` is false. Shown by `mix doctor` (T47).
    pub missing: Option<String>,
}

impl PortAccessState {
    /// What would have to be granted for [`bindings`](Self::bindings) to work, or [`None`] when
    /// nothing would.
    ///
    /// **Here rather than in the daemon**, because turning a method into an operation is the one
    /// piece of that decision that knows what each system's mechanism is, and the rule is that no
    /// such knowledge leaves this crate. The caller reads one function and never a `#[cfg]`.
    ///
    /// `binary` is the program that would hold the capability, and is ignored where the method is
    /// not [`PortAccessMethod::Capability`].
    #[must_use]
    pub fn plan(&self, binary: &Path) -> Option<PortAccessPlan> {
        match self.method {
            PortAccessMethod::Direct => None,

            PortAccessMethod::Capability => {
                let ports: Vec<u16> = self
                    .bindings
                    .iter()
                    .map(|binding| binding.answer)
                    .filter(|port| *port < FIRST_UNRESERVED)
                    .collect();

                (!ports.is_empty()).then(|| PortAccessPlan::Capability {
                    binary: binary.to_path_buf(),
                    ports,
                })
            }

            PortAccessMethod::Redirect => {
                let redirects: Vec<PortRedirect> = self
                    .bindings
                    .iter()
                    .filter(|binding| binding.answer != binding.bind)
                    .map(|binding| PortRedirect {
                        answer: binding.answer,
                        bind: binding.bind,
                    })
                    .collect();

                (!redirects.is_empty()).then_some(PortAccessPlan::Redirect { redirects })
            }
        }
    }

    /// What would have to be removed to undo [`plan`](Self::plan), or [`None`] on a system that
    /// grants nothing.
    ///
    /// **Nothing in T42 enqueues one** — the T42 design, D12. It exists so uninstall (T87) has a
    /// value to build rather than a reversal to invent against a grant written five phases earlier.
    #[must_use]
    pub fn target(&self, binary: &Path) -> Option<PortAccessTarget> {
        match self.method {
            PortAccessMethod::Direct => None,
            PortAccessMethod::Capability => Some(PortAccessTarget::Capability {
                binary: binary.to_path_buf(),
            }),
            PortAccessMethod::Redirect => Some(PortAccessTarget::Redirect {}),
        }
    }
}

/// Whether an unprivileged program may answer on a port the OS reserves — roadmap task **T42**.
///
/// **Not [`PortOwner`](crate::PortOwner)**, whose own documentation already draws the line: that one
/// is about who got to a port first, and this one is about being allowed to bind one at all.
///
/// **Reads only, and never prompts.** The write needs a token this process does not have; it is
/// [`PortAccessGrant`](mixengine_proto::privileged::PrivilegedOp::PortAccessGrant), applied by
/// `mixengine-elevate`. Reading needs no privilege on any of the three systems, which is what makes
/// the daemon able to ask on every start — and that, rather than a hook in the updater, is what
/// catches a capability lost when the binary was replaced.
pub trait PortAccess: std::fmt::Debug + Send + Sync {
    /// What this machine needs before `answering` can be served, and whether it is already there.
    ///
    /// `binary` is the program that would hold the grant, and is consulted only where the method is
    /// [`PortAccessMethod::Capability`].
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when a file this reads cannot be read, and
    /// [`Error::UnsupportedPlatform`](crate::Error::UnsupportedPlatform) where the machine offers no
    /// way to ask. **Every caller treats an error as "no answer" and carries on**: this is asked at
    /// start-up, and a probe that failed must not be the thing that stops a daemon.
    fn probe(&self, binary: &Path, answering: &[u16]) -> Result<PortAccessState>;
}
