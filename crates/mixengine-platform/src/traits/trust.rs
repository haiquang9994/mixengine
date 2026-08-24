//! Whether this machine trusts MixEngine's own certificate authority.

use mixengine_proto::privileged::{TrustPlan, TrustTarget};

use crate::Result;

/// How this machine can be made to trust a certificate authority of our own.
///
/// **Windows and macOS are constants; Linux is a question asked at runtime** — the T49a design, D7,
/// on [`ResolverMethod`](crate::ResolverMethod)'s precedent and for its reason. A Debian-family
/// machine takes an anchor in one directory and a Red Hat one takes it in another, each refreshed by
/// its own command, and a machine with neither has no system store MixEngine knows how to write.
/// That is why [`TrustStore::method`] returns a `Result` where
/// [`PortAccessMethod`](crate::PortAccessMethod)'s equivalent is a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustStoreMethod {
    /// Windows: `Root`, under `LocalMachine`.
    SystemRoot,

    /// macOS: `/Library/Keychains/System.keychain`.
    SystemKeychain,

    /// Linux, Debian family: `/usr/local/share/ca-certificates`.
    CaCertificates,

    /// Linux, Red Hat family: `/etc/pki/ca-trust/source/anchors`.
    CaTrustAnchors,

    /// This machine has no system trust store MixEngine knows how to write.
    ///
    /// **A valid answer, not an error**, exactly as [`ResolverMethod::None`](crate::ResolverMethod)
    /// is. Sites keep working over HTTP, `cert.ca_status` says why in words, and nothing fails —
    /// which is what `.claude/features/tls.md` asks for, with the correction that what is
    /// unsupported is this machine's configuration rather than the platform.
    None,
}

/// What this machine trusts today, and what it would take to trust ours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustState {
    /// Which mechanism this machine has.
    pub method: TrustStoreMethod,

    /// Whether the certificate that was asked about is already there.
    ///
    /// **Compared as exact DER bytes** — the T49a design, D6. A subject match would claim another
    /// home's authority as this one's, and a store's own SHA-1 property is a different value from
    /// the SHA-256 `cert.ca_status` reports, so carrying two hashes for one identity is how they
    /// come apart.
    pub installed: bool,

    /// Why not, in words, for `mix doctor` (T49a's `CaNotTrusted`) and for the screen.
    pub missing: Option<String>,
}

impl TrustState {
    /// What would have to be applied for this machine to trust `der`, or [`None`] when nothing
    /// would.
    ///
    /// **Whole state, like every operation beside it**: a machine that already holds exactly this
    /// certificate plans nothing, so no prompt is spent on a row whose only possible outcome is
    /// `AlreadyDone` — T41's D11, two capabilities along.
    #[must_use]
    pub fn plan(&self, der: &[u8]) -> Option<TrustPlan> {
        if self.installed {
            return None;
        }

        match self.method {
            TrustStoreMethod::None => None,
            TrustStoreMethod::SystemRoot => Some(TrustPlan::SystemRoot { der: der.to_vec() }),
            TrustStoreMethod::SystemKeychain => {
                Some(TrustPlan::SystemKeychain { der: der.to_vec() })
            }
            TrustStoreMethod::CaCertificates => {
                Some(TrustPlan::CaCertificates { der: der.to_vec() })
            }
            TrustStoreMethod::CaTrustAnchors => {
                Some(TrustPlan::CaTrustAnchors { der: der.to_vec() })
            }
        }
    }

    /// What would have to be removed to undo [`plan`](Self::plan), or [`None`] on a machine with no
    /// store.
    ///
    /// **It names an authority and never a certificate** — the T49a design, D5. `key_id` is T48's
    /// eight hex characters; there is no fingerprint here, because a removal that could name one
    /// could name the root that validates this machine's own updates.
    ///
    /// **Nothing in T49a calls this** — D5, on T42's D12 and T45's D13. It exists now so that T54
    /// and T87 have a value to build against rather than a reversal to invent against a mechanism
    /// written phases earlier.
    #[must_use]
    pub fn target(&self, key_id: &str) -> Option<TrustTarget> {
        let key_id = key_id.to_owned();

        match self.method {
            TrustStoreMethod::None => None,
            TrustStoreMethod::SystemRoot => Some(TrustTarget::SystemRoot { key_id }),
            TrustStoreMethod::SystemKeychain => Some(TrustTarget::SystemKeychain { key_id }),
            TrustStoreMethod::CaCertificates => Some(TrustTarget::CaCertificates { key_id }),
            TrustStoreMethod::CaTrustAnchors => Some(TrustTarget::CaTrustAnchors { key_id }),
        }
    }
}

/// Whether this machine trusts MixEngine's own certificate authority — roadmap task **T49a**.
///
/// **Reads only, and never prompts.** The write needs a token this process does not have; it is
/// `PrivilegedOp::TrustCaInstall`, applied by `mixengine-elevate` — carrying a
/// [`TrustPlan`] this state built. Reading needs no privilege on any of the three systems, which is what makes
/// it affordable to ask on every daemon start — and `tests/trust.rs` is what measures that rather
/// than this sentence.
pub trait TrustStore: std::fmt::Debug + Send + Sync {
    /// Which mechanism this machine has, or [`TrustStoreMethod::None`].
    ///
    /// # Errors
    ///
    /// [`Error::Io`](crate::Error::Io) when something this reads cannot be read. **Every caller
    /// treats an error as "no answer" and carries on**: this is asked at start-up, and a probe that
    /// failed must not become the thing that stops a daemon.
    fn method(&self) -> Result<TrustStoreMethod>;

    /// Whether this machine already holds exactly `der`, and why not.
    ///
    /// **This answers "is the certificate in the store", not "does a browser trust it"**, and the
    /// difference is the one [`ResolverConfig::probe`](crate::ResolverConfig::probe) draws for its
    /// own capability: Firefox and Chrome on Linux read NSS and not this store at all (T49b), and a
    /// browser already running may not re-read a store it has cached. The honest end-to-end check is
    /// a live TLS handshake, which is `mix cert status`' job (T53).
    ///
    /// # Errors
    ///
    /// As [`method`](Self::method).
    fn probe(&self, der: &[u8]) -> Result<TrustState>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(method: TrustStoreMethod, installed: bool) -> TrustState {
        TrustState {
            method,
            installed,
            missing: None,
        }
    }

    /// A machine that already trusts it asks for nothing, so no prompt is spent on a row whose only
    /// possible outcome is `AlreadyDone`.
    #[test]
    fn a_machine_that_already_trusts_it_needs_no_plan() {
        assert_eq!(
            state(TrustStoreMethod::SystemRoot, true).plan(&[1, 2, 3]),
            None
        );
    }

    #[test]
    fn a_machine_that_does_not_is_asked_for_this_certificate() {
        assert_eq!(
            state(TrustStoreMethod::SystemRoot, false).plan(&[1, 2, 3]),
            Some(TrustPlan::SystemRoot { der: vec![1, 2, 3] })
        );
    }

    /// Each mechanism asks for its own, and never for another system's.
    #[test]
    fn every_mechanism_plans_its_own_store() {
        for (method, expected) in [
            (
                TrustStoreMethod::SystemKeychain,
                TrustPlan::SystemKeychain { der: vec![7] },
            ),
            (
                TrustStoreMethod::CaCertificates,
                TrustPlan::CaCertificates { der: vec![7] },
            ),
            (
                TrustStoreMethod::CaTrustAnchors,
                TrustPlan::CaTrustAnchors { der: vec![7] },
            ),
        ] {
            assert_eq!(state(method, false).plan(&[7]), Some(expected));
        }
    }

    /// D7: a machine with no store is a supported machine, not a failure.
    #[test]
    fn a_machine_with_no_store_asks_for_nothing_in_either_direction() {
        let state = state(TrustStoreMethod::None, false);

        assert_eq!(state.plan(&[1]), None);
        assert_eq!(state.target("deadbeef"), None);
    }

    /// D5, at the layer that builds the value: an authority is named and a certificate is not.
    #[test]
    fn a_target_names_the_authority_and_the_mechanism() {
        assert_eq!(
            state(TrustStoreMethod::CaCertificates, true).target("deadbeef"),
            Some(TrustTarget::CaCertificates {
                key_id: "deadbeef".to_owned()
            })
        );
    }

    /// A machine that already trusts it can still be asked to stop, which is what makes the two
    /// directions independent rather than a toggle.
    #[test]
    fn a_target_does_not_depend_on_whether_it_is_installed() {
        assert_eq!(
            state(TrustStoreMethod::SystemRoot, true).target("deadbeef"),
            state(TrustStoreMethod::SystemRoot, false).target("deadbeef"),
        );
    }
}
