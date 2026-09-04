//! `daemon.uninstall_plan` and `daemon.uninstall` — roadmap task **T87**.
//!
//! **Two methods on `daemon.doctor`/`daemon.doctor_repair`'s split, and not one flag.** The plan is
//! a read in the strict sense — no row written, nothing enqueued, no prompt possible — which is what
//! makes it safe to put in front of the one command that cannot be undone. The act is a job, because
//! it can raise the elevation prompt and what that waits on is a person reading a dialog.
//!
//! Both build their list from [`inventory::take`], which is the one enumeration: two of them, one
//! for the dry run and one for the real run, is the second inventory the roadmap sentence refuses.

mod inventory;

use std::sync::Arc;

use mixengine_proto::{Error, Residue, UninstallQuery, UninstallReport};

/// Both halves of the uninstall.
///
/// **Holding one set of readers**, so the plan and the act cannot disagree about what is on this
/// machine — and the same readers `Doctor` is given, reached through the door that already owns
/// each. A second `Host` here would be a second answer to *what does this machine's hosts file
/// hold*.
#[derive(Debug)]
pub(crate) struct Uninstall {
    /// The rows, for the one question this asks of them: is anything shared?
    store: mixengine_core::Store,

    /// This machine: its hosts file, its resolver, its trust store, its port access, its browsers.
    host: Arc<dyn mixengine_platform::Host>,

    /// Where the DNS server is listening, which is what a resolver would have been pointed at.
    dns: Arc<crate::dns::Dns>,

    /// The front end's program path, which is what the port-access reading is about.
    services: Arc<crate::services::Registry>,

    /// `<root>/bin` and this user's PATH — the first of the two things outside the home that need no
    /// token.
    shims: Arc<crate::shims::Shims>,

    /// The login entry — the second of them.
    autostart: Arc<crate::autostart::Autostart>,

    /// The home's own layout: where its authority is, and every directory it owns.
    paths: mixengine_core::Paths,
}

impl Uninstall {
    /// The one of these the API holds.
    ///
    /// **The home's layout rather than its root and its certificate directory separately**, on
    /// `Doctor::new`'s rule: both are derived from it, and passing them apart lets a caller hand
    /// this a certificate directory from one home and a root from another.
    pub(crate) fn new(
        store: &mixengine_core::Store,
        host: Arc<dyn mixengine_platform::Host>,
        dns: Arc<crate::dns::Dns>,
        services: Arc<crate::services::Registry>,
        shims: Arc<crate::shims::Shims>,
        autostart: Arc<crate::autostart::Autostart>,
        paths: &mixengine_core::Paths,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: store.clone(),
            host,
            dns,
            services,
            shims,
            autostart,
            paths: paths.clone(),
        })
    }

    /// `daemon.uninstall_plan` — everything an uninstall would take off this machine.
    ///
    /// **A read, and every branch of it.** Nothing here writes a row, enqueues an operation or can
    /// raise a prompt, which is what `daemon.doctor` established the shape for and what makes this
    /// safe to call from a client that is only showing somebody what would happen.
    ///
    /// # Errors
    ///
    /// The wire error of a home whose layout could not be read. A *machine* that could not be read
    /// is never an error: it is [`Removal::Failed`](mixengine_proto::Removal::Failed) on the row it
    /// is about, because an uninstall that reported "nothing there" for a question nobody could
    /// answer is the one failure this feature exists to prevent.
    pub(crate) async fn plan(&self, query: &UninstallQuery) -> Result<UninstallReport, Error> {
        Ok(UninstallReport {
            items: self.rows(query).await?,
            // Never anything else: a plan raises no prompt, so there is no grant to report.
            granting: None,
        })
    }

    /// The inventory, taken once.
    async fn rows(&self, query: &UninstallQuery) -> Result<Vec<Residue>, Error> {
        inventory::take(self, query).await
    }
}
