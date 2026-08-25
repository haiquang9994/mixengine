//! `site.*`: what is served out of a project's directory, and at what name.
//!
//! Roadmap task **T39a**. [`crate::projects`]' shape one table down, and nothing more: this task
//! declares, and T43 serves. No config file is rendered here and no process is started — but a
//! declared name has to resolve, so every write asks [`crate::elevation`] for the hosts file this
//! home now needs (T41). Asking is all it does: the operation waits for somebody to allow it.
//!
//! # A create is also the import
//!
//! [`ProjectCreate`](mixengine_proto::ProjectCreate)'s rule, one table down (spec D7): every field
//! but the project falls through — the argument, then `[site]` in the project's manifest, then a
//! default. `site.create { project }` with nothing else typed is therefore how a colleague's
//! checkout gets its site, and there is one code path rather than two for one outcome.
//!
//! # Two answers about a pool, because the row holds one of them
//!
//! A php-fpm site's pool is frozen at create while the project's shell keeps following the default.
//! [`mixengine_proto::SitePool`] reports `declared` — what the row holds — beside
//! `resolved` — what `core::resolve` answers at that root today — so somebody whose site is on
//! 8.3.34 while their shell is on 8.3.35 can see that rather than guess at it.
//!
//! # The order of the checks
//!
//! `api/create.rs`' order and its reasoning: cheapest and most specific first, and nothing is
//! written until every one of them has passed. The project, then the doc root, then the domains,
//! then the kind's payload, then the services, then the pool — and only then the write, whose
//! unique index has the last word about whether a domain was free.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mixengine_core::{Store, domains, manifest, projects, resolve, services, sites};
use mixengine_proto::{
    Error, ErrorCode, ProjectRef, RuntimeKind, ServiceId, SiteCreate, SiteCreation, SiteDetail,
    SiteKind, SiteList, SiteListQuery, SitePool, SiteQuery, SiteRef, SiteRemoval, SiteServiceLink,
    SiteState, SiteSummary, SiteUpdate,
};

use crate::error::ToWire as _;

/// Everything `site.*` needs: the rows, the queue a name change has to reach, and the registry that
/// renders what a site is served by.
#[derive(Debug)]
pub(crate) struct Sites {
    /// Where a site is written down.
    store: Store,

    /// Where a change to the names this home answers for goes to wait for permission — T41.
    elevation: Arc<crate::elevation::Elevation>,

    /// What turns the rows into the front end's configuration, and tells the running server —
    /// roadmap task **T43**. Every write below ends here.
    services: Arc<crate::services::Registry>,

    /// What signs this site's certificate — roadmap task **T50**.
    ///
    /// **Held here rather than reached for through the API**, so a site gains its certificate
    /// before `site.create` answers: a site that needed a second command to get a padlock is a site
    /// whose padlock is somebody's to remember. `Certificates::issue` takes a row and never a
    /// [`SiteRef`], which is what keeps this a one-way edge — a `Certificates` that resolved
    /// references would need the `Sites` holding it.
    certificates: crate::certs::Certificates,
}

impl Sites {
    /// The one of these the API holds.
    pub(crate) fn new(
        store: &Store,
        elevation: Arc<crate::elevation::Elevation>,
        services: Arc<crate::services::Registry>,
        paths: &mixengine_core::Paths,
    ) -> Arc<Self> {
        Arc::new(Self {
            certificates: crate::certs::Certificates::issuing(
                paths,
                elevation.host(),
                store.clone(),
            ),
            store: store.clone(),
            elevation,
            services,
        })
    }

    /// Give this site the certificate its names need — roadmap task **T50**.
    ///
    /// **Before the walk that renders its configuration and never after**, which is the ordering
    /// the T50 design's D1 argues for: `etc/` is disposable and rebuilt from the rows, a certificate
    /// is state that cannot be, so issuance is a precondition of generation rather than a step in
    /// it. T51 is what will make the difference visible.
    ///
    /// **Run after an update as well as a create**, because a site that just gained a domain has a
    /// certificate that no longer covers its names — T50's second reuse question. That is what
    /// `.claude/features/tls.md` names as the most common "the padlock broke" report.
    ///
    /// A failure is logged and never returned, exactly as [`Self::wants_the_hosts_file`] is: the
    /// row is already written by the time this runs, a site that exists is worth more than a
    /// certificate that does not, and `mix doctor` reports the gap.
    async fn now_has_a_certificate(&self, site: &sites::SiteRecord) {
        let domain = site.domains.first().cloned().unwrap_or_default();

        match self.certificates.issue(Some(site.clone())).await {
            Ok(report) => {
                for outcome in report.sites {
                    match outcome.outcome {
                        mixengine_proto::IssueOutcome::Refused { because } => {
                            tracing::warn!(%domain, %because, "the site has no certificate yet");
                        }

                        // **Silence and not a warning** — roadmap task **T52**. A site with HTTPS
                        // off asked for no certificate, and saying it has none is a line whoever
                        // reads this log has to work out is not about them. Before T52 split the
                        // outcome, every plaintext site created wrote one.
                        mixengine_proto::IssueOutcome::NotWanted { .. }
                        | mixengine_proto::IssueOutcome::Issued {}
                        | mixengine_proto::IssueOutcome::Reused {} => {}
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%domain, %error, "the site has no certificate yet");
            }
        }
    }

    /// Queue the hosts change this home now needs, and never fail an operation over it.
    ///
    /// The site is already written by the time this runs. Returning an error here would say the
    /// operation failed when it did not, and the queue is a want rather than a step: `mix status`
    /// keeps showing what is waiting until somebody grants it or drops it.
    async fn wants_the_hosts_file(&self) {
        if let Err(error) = self.elevation.require_hosts().await {
            tracing::warn!(
                ?error,
                "the site was written, but nothing was queued for the hosts file"
            );
        }
    }

    /// Render what this home now declares, and tell the front end — roadmap task **T43**, D10.
    ///
    /// **Unlike [`wants_the_hosts_file`](Self::wants_the_hosts_file), a failure here fails the
    /// call**, and the difference is what a failure *means*. A hosts entry that has not been granted
    /// is a want with a person on the other end of it, and `mix status` keeps saying so. A
    /// configuration the server refused is a defect: nothing was installed, the front end is still
    /// reading the configuration that worked, and a `site.create` that answered success would send
    /// whoever typed it to look in the wrong place.
    ///
    /// **The row is already written by the time this runs.** That is deliberate rather than
    /// unavoidable — a site is declared state, and a declaration rolled back because the rendering
    /// failed would leave a person with nothing to fix. What they get is a site they can see in
    /// `mix site list` and an error naming the file the checker complained about.
    ///
    /// A home with no front end renders nothing and this succeeds: there is no set to add sites to,
    /// which is a different thing from a set that was refused.
    ///
    /// # Errors
    ///
    /// Whatever the walk reports — a rendering that failed, or a set that is not a graph.
    async fn now_serves_what_it_declares(&self) -> Result<(), Error> {
        self.services
            .reconfigure()
            .await
            .map_err(|error| error.to_wire())
    }

    /// `site.create` — declare a site under a project, taking what it was not told from `[site]`.
    ///
    /// # Errors
    ///
    /// `not_found` for a project matching nothing, a service that is not declared, and a pool that
    /// resolves to nothing; `invalid_argument` for a domain that is not one, a doc root outside the
    /// root, and a kind whose payload is wrong; `already_exists` for a domain another site owns.
    pub(crate) async fn create(&self, create: &SiteCreate) -> Result<SiteCreation, Error> {
        let project = self.project(&create.project).await?;
        let manifest =
            manifest::read(&manifest::at(&project.root)).map_err(|error| error.to_wire())?;
        let declared = manifest
            .as_ref()
            .and_then(|manifest| manifest.site.as_ref());

        // The fall-through, in the order spec D7 writes it: the argument, the manifest, the default.
        let domains = match &create.domains {
            Some(domains) => domains.clone(),
            None => match declared.and_then(|site| site.domain.clone()) {
                Some(domain) => std::iter::once(domain)
                    .chain(
                        declared
                            .map(|site| site.aliases.clone())
                            .unwrap_or_default(),
                    )
                    .collect(),
                None => vec![domains::default_for(&project.name).map_err(|e| e.to_wire())?],
            },
        };

        let doc_root = create
            .doc_root
            .clone()
            .or_else(|| declared.and_then(|site| site.doc_root.clone()))
            .unwrap_or_default();

        let https = create
            .https
            .or_else(|| declared.and_then(|site| site.https))
            .unwrap_or(true);

        let kind = create
            .kind
            .clone()
            .or_else(|| declared.and_then(|site| site.kind.clone()))
            .unwrap_or(SiteKind::PhpFpm { pool: None });

        let services = match &create.services {
            Some(services) => services.clone(),
            None => self.linked(manifest.as_ref()).await?,
        };

        let new = sites::NewSite {
            project_id: project.id,
            doc_root: sites::relative_doc_root(&project.root, &doc_root)
                .map_err(|error| error.to_wire())?,
            kind: self.settled(&kind, &project).await?,
            https_enabled: https,
            domains: self.checked(&domains, create.accept_risky_tld)?,
            services: self.existing(&services).await?,
        };

        let written = sites::create(&self.store, &new)
            .await
            .map_err(|error| error.to_wire())?;

        self.wants_the_hosts_file().await;
        self.now_has_a_certificate(&written).await;
        self.now_serves_what_it_declares().await?;

        self.detail(&written, &project)
            .await
            .map(|site| SiteCreation { site })
    }

    /// `site.list` — every site, or one project's.
    ///
    /// # Errors
    ///
    /// `not_found` for a project matching nothing, and the wire error of a table that cannot be
    /// read.
    pub(crate) async fn list(&self, query: &SiteListQuery) -> Result<SiteList, Error> {
        let project = match &query.project {
            Some(reference) => Some(self.project(reference).await?),
            None => None,
        };

        let records = sites::records(&self.store, project.as_ref().map(|found| found.id))
            .await
            .map_err(|error| error.to_wire())?;

        let mut listed = Vec::with_capacity(records.len());

        for record in records {
            let owner = match &project {
                Some(project) => project.clone(),
                None => self.project_by_id(record.project_id).await?,
            };

            listed.push(summary(&record, &owner));
        }

        Ok(SiteList { sites: listed })
    }

    /// `site.show` — one site, with its domains, its pool and its services.
    ///
    /// # Errors
    ///
    /// `not_found` for a reference matching nothing, and `invalid_argument` for a path whose
    /// project holds several sites.
    pub(crate) async fn show(&self, query: &SiteQuery) -> Result<SiteDetail, Error> {
        let (site, project) = self.expect(&query.site).await?;

        self.detail(&site, &project).await
    }

    /// `site.update` — change what a site is.
    ///
    /// `domains` and `services` **replace** rather than merge: with a merge there is no way to
    /// remove one.
    ///
    /// # Errors
    ///
    /// Everything [`Sites::create`] refuses, plus `not_found` for a site matching nothing.
    /// Set this site's domains to exactly this list — roadmap task **T46**.
    ///
    /// [`Self::update`] with one field filled in, named so that the two `domain.*` verbs read as
    /// what they are rather than as an update that happens to leave six fields empty — and so that
    /// there is one place to look when the shape of an update changes.
    ///
    /// # Errors
    ///
    /// [`Self::update`]'s: an unknown site, a public TLD, `.local` without the acknowledgement, a
    /// domain another site already holds.
    pub(crate) async fn replace_domains(
        &self,
        site: &SiteRef,
        domains: Vec<String>,
        accept_risky_tld: bool,
    ) -> Result<SiteDetail, Error> {
        self.update(&SiteUpdate {
            site: site.clone(),
            domains: Some(domains),
            accept_risky_tld,
            doc_root: None,
            kind: None,
            services: None,
            https: None,
            state: None,
        })
        .await
    }

    pub(crate) async fn update(&self, update: &SiteUpdate) -> Result<SiteDetail, Error> {
        let (site, project) = self.expect(&update.site).await?;

        let kind = match &update.kind {
            Some(kind) => Some(self.settled(kind, &project).await?),
            None => None,
        };

        let domains = match &update.domains {
            Some(domains) => Some(self.checked(domains, update.accept_risky_tld)?),
            None => None,
        };

        let services = match &update.services {
            Some(services) => Some(self.existing(services).await?),
            None => None,
        };

        let doc_root = match &update.doc_root {
            Some(doc_root) => {
                Some(sites::relative_doc_root(&project.root, doc_root).map_err(|e| e.to_wire())?)
            }
            None => None,
        };

        let changed = sites::update(
            &self.store,
            site.id,
            &sites::Change {
                doc_root,
                kind,
                https_enabled: update.https,
                state: update.state,
                domains,
                services,
            },
        )
        .await
        .map_err(|error| error.to_wire())?;

        self.wants_the_hosts_file().await;
        self.now_has_a_certificate(&changed).await;
        self.now_serves_what_it_declares().await?;

        self.detail(&changed, &project).await
    }

    /// `site.start` — serve this site.
    ///
    /// **A flag and a walk** — D9. The state is set, the front end's configuration is re-rendered
    /// and the running server is told; nothing starts a process. A home whose Caddy is not running
    /// gets a rendered site file and no traffic, which is T39a's line held rather than a limitation:
    /// a `site.start` that started the front end would owe an answer to what it means when the pool
    /// starts and the front end does not.
    ///
    /// **Idempotent, and not because anything here checks.** A second call on an enabled site
    /// re-renders the same bytes; `document::install` compares before it stages, finds nothing
    /// changed, skips the validator and reloads nothing.
    ///
    /// # Errors
    ///
    /// `not_found` for a site matching nothing, `invalid_argument` for a path whose project holds
    /// several sites, and whatever the walk reports for a configuration the front end refuses.
    pub(crate) async fn start(&self, query: &SiteQuery) -> Result<SiteDetail, Error> {
        self.serving(query, SiteState::Enabled).await
    }

    /// `site.stop` — keep the declaration, stop serving it.
    ///
    /// The site's rendered file goes on the same walk, because the front end's `sites/` directory
    /// holds exactly what was rendered into it. Without that, a stopped site would go on being
    /// served until something else changed.
    ///
    /// # Errors
    ///
    /// [`Sites::start`]'s.
    pub(crate) async fn stop(&self, query: &SiteQuery) -> Result<SiteDetail, Error> {
        self.serving(query, SiteState::Disabled).await
    }

    /// Both of the above: set the state, walk, and answer with the site as it now is.
    ///
    /// The same answer as `site.show`, because what a caller wants back is the site as it now
    /// stands — not a confirmation that something happened to it.
    async fn serving(&self, query: &SiteQuery, state: SiteState) -> Result<SiteDetail, Error> {
        let (site, project) = self.expect(&query.site).await?;

        let changed = sites::update(
            &self.store,
            site.id,
            &sites::Change {
                state: Some(state),
                ..sites::Change::default()
            },
        )
        .await
        .map_err(|error| error.to_wire())?;

        self.now_serves_what_it_declares().await?;

        self.detail(&changed, &project).await
    }

    /// `site.delete` — take the row, and leave the files.
    ///
    /// # Errors
    ///
    /// `not_found` for a site matching nothing, and the wire error of a row that cannot be removed.
    pub(crate) async fn delete(&self, query: &SiteQuery) -> Result<SiteRemoval, Error> {
        let (removed, project) = self.expect(&query.site).await?;

        sites::delete(&self.store, removed.id)
            .await
            .map_err(|error| error.to_wire())?;

        self.wants_the_hosts_file().await;
        self.now_serves_what_it_declares().await?;

        Ok(SiteRemoval {
            domains_released: removed.domains.clone(),
            doc_root_kept: doc_root_full(&project.root, &removed.doc_root)
                .display()
                .to_string(),
            removed: summary(&removed, &project),
        })
    }

    /// The site a reference names, or the refusal for one that names nothing.
    ///
    /// **A `Path` that reaches a project with several sites is refused, naming them** (spec D5):
    /// picking one would be picking at random, and the person typing it in a shell has a domain to
    /// hand.
    ///
    /// # Errors
    ///
    /// `not_found` for a domain nothing answers to and a directory under no project;
    /// `invalid_argument` for a directory whose project holds more than one site.
    pub(crate) async fn expect(
        &self,
        reference: &SiteRef,
    ) -> Result<(sites::SiteRecord, projects::ProjectRecord), Error> {
        match reference {
            SiteRef::Domain(domain) => {
                let found = sites::by_domain(&self.store, &domain.to_ascii_lowercase())
                    .await
                    .map_err(|error| error.to_wire())?
                    .ok_or_else(|| {
                        Error::new(ErrorCode::NotFound, format!("no site answers to {domain}"))
                            .with_hint("`mix site list` shows what does")
                    })?;

                let project = self.project_by_id(found.project_id).await?;

                Ok((found, project))
            }

            SiteRef::Path(path) => {
                let project = self.project(&ProjectRef::Path(path.clone())).await?;
                let mut found = sites::records(&self.store, Some(project.id))
                    .await
                    .map_err(|error| error.to_wire())?;

                match found.len() {
                    0 => Err(Error::new(
                        ErrorCode::NotFound,
                        format!("{} has no site", project.name),
                    )
                    .with_hint(format!("`mix site create {}` declares one", project.name))),

                    1 => Ok((found.remove(0), project)),

                    _ => {
                        let named = found
                            .iter()
                            .filter_map(|site| site.domains.first())
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", ");

                        Err(Error::new(
                            ErrorCode::InvalidArgument,
                            format!("{} has more than one site: {named}", project.name),
                        )
                        .with_hint("name one of those domains"))
                    }
                }
            }
        }
    }

    /// Every domain, normalised and checked, with a repeat inside one request refused before the
    /// table is asked — a unique-index violation would blame whichever came second.
    fn checked(&self, domains: &[String], risky: bool) -> Result<Vec<String>, Error> {
        if domains.is_empty() {
            return Err(Error::new(
                ErrorCode::InvalidArgument,
                "a site needs at least one domain",
            )
            .with_hint("`--domain blog.test`, or leave it out and take `<project>.test`"));
        }

        let mut seen = std::collections::BTreeSet::new();
        let mut checked = Vec::with_capacity(domains.len());

        for domain in domains {
            let name = domains::normalised(domain, risky).map_err(|error| error.to_wire())?;

            if !seen.insert(name.clone()) {
                return Err(Error::new(
                    ErrorCode::InvalidArgument,
                    format!("{name} is named twice in one request"),
                ));
            }

            checked.push(name);
        }

        Ok(checked)
    }

    /// The kind with its pool decided and its payload checked.
    ///
    /// A php-fpm site that named no pool gets the one `core::resolve` answers at the project's root
    /// — pools are `php-fpm@<full version>`, so `^8.3` becomes `php-fpm@8.3.34`. Frozen there, on
    /// purpose: what a shell resolves tomorrow is not what a site was declared with.
    async fn settled(
        &self,
        kind: &SiteKind,
        project: &projects::ProjectRecord,
    ) -> Result<SiteKind, Error> {
        match kind {
            SiteKind::PhpFpm { pool: Some(pool) } => {
                self.existing(std::slice::from_ref(pool)).await?;

                Ok(kind.clone())
            }

            SiteKind::PhpFpm { pool: None } => {
                let resolved = resolve::runtime(
                    &self.store,
                    &resolve::Question {
                        kind: RuntimeKind::Php,
                        cwd: Some(&project.root),
                        explicit: None,
                    },
                )
                .await
                .map_err(|error| error.to_wire())?;

                let pool = ServiceId::parse(format!("php-fpm@{}", resolved.runtime.version))
                    .map_err(|error| {
                        Error::new(ErrorCode::Internal, format!("{error}")).with_hint(
                            "a resolved PHP version does not spell a service id, which is a bug",
                        )
                    })?;

                Ok(SiteKind::PhpFpm { pool: Some(pool) })
            }

            SiteKind::ReverseProxy { upstream } => {
                upstream_is_an_address(upstream)?;

                Ok(kind.clone())
            }

            SiteKind::Static | SiteKind::NodeApp { .. } => Ok(kind.clone()),
        }
    }

    /// The project a reference names, or `not_found`.
    async fn project(&self, reference: &ProjectRef) -> Result<projects::ProjectRecord, Error> {
        projects::find(&self.store, reference)
            .await
            .map_err(|error| error.to_wire())?
            .ok_or_else(|| {
                let (said, hint) = match reference {
                    ProjectRef::Name(name) => (
                        format!("no such project: {name}"),
                        "`mix project list` shows what does exist".to_owned(),
                    ),
                    ProjectRef::Path(path) => (
                        format!("no project is registered at or above {path}"),
                        format!("`mix project create {path}` registers it"),
                    ),
                };

                Error::new(ErrorCode::NotFound, said).with_hint(hint)
            })
    }

    /// The project a row points at.
    ///
    /// `internal` when it is gone, which the cascade makes impossible and which is therefore a bug
    /// rather than a user's mistake.
    async fn project_by_id(&self, id: i64) -> Result<projects::ProjectRecord, Error> {
        projects::records(&self.store)
            .await
            .map_err(|error| error.to_wire())?
            .into_iter()
            .find(|project| project.id == id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Internal,
                    format!("site {id} belongs to a project that is not there"),
                )
                .with_hint("deleting a project takes its sites, so this is a bug in MixEngine")
            })
    }

    /// Every id checked to exist, answered in the order it was given.
    ///
    /// A miss is `not_found` carrying `` `mix service create <id>` declares it ``, on an
    /// uninstalled runtime's precedent: the refusal names the command that fixes it.
    async fn existing(&self, wanted: &[ServiceId]) -> Result<Vec<ServiceId>, Error> {
        for service in wanted {
            services::record(&self.store, service).await.map_err(|_| {
                Error::new(ErrorCode::NotFound, format!("no such service: {service}"))
                    .with_hint(format!("`mix service create {service}` declares it"))
            })?;
        }

        Ok(wanted.to_vec())
    }

    /// The manifest's `[[services]]` as ids.
    ///
    /// **An absent `instance` is a lookup, not a second identity**: the bare `name` when a service
    /// by that id exists — which is what a single-instance package such as `caddy` is called — and
    /// `name@main` otherwise.
    ///
    /// A `version` nothing installed satisfies is **not** refused (spec D8): refusing an import
    /// because this machine has MariaDB 11.5 would break the clean-machine case the import path
    /// exists for. It is written to `daemon.log` at `info` and no further — there is no field on
    /// [`SiteDetail`] for it, and inventing one here would be an API this spec did not agree.
    async fn linked(&self, manifest: Option<&manifest::Manifest>) -> Result<Vec<ServiceId>, Error> {
        let Some(manifest) = manifest else {
            return Ok(Vec::new());
        };

        let mut linked = Vec::with_capacity(manifest.services.len());

        for declared in &manifest.services {
            let spelled = match &declared.instance {
                Some(instance) => vec![format!("{}@{}", declared.name, instance)],
                None => vec![declared.name.clone(), format!("{}@main", declared.name)],
            };

            let mut found = None;

            for candidate in spelled {
                let Ok(id) = ServiceId::parse(candidate) else {
                    continue;
                };

                if services::record(&self.store, &id).await.is_ok() {
                    found = Some(id);
                    break;
                }
            }

            let id = found.ok_or_else(|| {
                Error::new(
                    ErrorCode::NotFound,
                    format!("the manifest declares {}, which is not here", declared.name),
                )
                .with_hint(format!(
                    "`mix service create {}` declares it, or drop it from mixengine.toml",
                    declared.name
                ))
            })?;

            if let Some(wanted) = &declared.version {
                tracing::info!(
                    service = %id,
                    constraint = %wanted.as_str(),
                    "a manifest asks for a version of a service; this build records the link and \
                     nothing about the version"
                );
            }

            linked.push(id);
        }

        Ok(linked)
    }

    /// One record as the wire describes it: the summary, the two pool answers, and the links with
    /// each service's current state.
    async fn detail(
        &self,
        site: &sites::SiteRecord,
        project: &projects::ProjectRecord,
    ) -> Result<SiteDetail, Error> {
        let full = doc_root_full(&project.root, &site.doc_root);

        let pool = match &site.kind {
            SiteKind::PhpFpm { pool } => {
                // Asked again rather than remembered, because the point of showing both is that
                // they can differ: the row was frozen at create and the resolver moves on.
                let resolved = resolve::runtime(
                    &self.store,
                    &resolve::Question {
                        kind: RuntimeKind::Php,
                        cwd: Some(&project.root),
                        explicit: None,
                    },
                )
                .await
                .ok()
                .and_then(|resolved| {
                    ServiceId::parse(format!("php-fpm@{}", resolved.runtime.version)).ok()
                });

                Some(SitePool {
                    declared: pool.clone(),
                    resolved,
                })
            }

            _ => None,
        };

        let mut linked = Vec::with_capacity(site.services.len());

        for service in &site.services {
            let state = services::record(&self.store, service)
                .await
                .map_err(|error| error.to_wire())?
                .state;

            linked.push(SiteServiceLink {
                service: service.clone(),
                state,
            });
        }

        Ok(SiteDetail {
            site: summary(site, project),
            root: project.root.display().to_string(),
            doc_root_full: full.display().to_string(),
            doc_root_exists: full.is_dir(),
            domains: site.domains.clone(),
            pool,
            services: linked,
        })
    }
}

/// One record, as a listing shows it.
fn summary(site: &sites::SiteRecord, project: &projects::ProjectRecord) -> SiteSummary {
    SiteSummary {
        domain: site.domains.first().cloned().unwrap_or_default(),
        project: project.name.clone(),
        kind: site.kind.clone(),
        doc_root: site.doc_root.clone(),
        https: site.https_enabled,
        state: site.state,
    }
}

/// Root plus doc root, as the filesystem spells it. `""` is the root itself.
fn doc_root_full(root: &Path, doc_root: &str) -> PathBuf {
    match doc_root.is_empty() {
        true => root.to_path_buf(),
        false => doc_root
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part)),
    }
}

/// A proxy target is an address: an absolute `http`/`https` URL with a host, a path allowed.
///
/// A query or a fragment is refused because a proxy target carrying one is a typo the renderer
/// would copy into a configuration file, where it would be silently ignored.
fn upstream_is_an_address(upstream: &str) -> Result<(), Error> {
    let refusal = |because: &str| {
        Error::new(
            ErrorCode::InvalidArgument,
            format!("{upstream} is not something to forward to: {because}"),
        )
        .with_hint("an address such as http://127.0.0.1:8080")
    };

    let (scheme, rest) = upstream
        .split_once("://")
        .ok_or_else(|| refusal("it has no scheme"))?;

    if !matches!(scheme, "http" | "https") {
        return Err(refusal("only http and https are forwarded to"));
    }

    if rest.contains('?') || rest.contains('#') {
        return Err(refusal("a query or a fragment is not part of an address"));
    }

    let host = rest.split('/').next().unwrap_or_default();

    if host.is_empty() {
        return Err(refusal("it has no host"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fourth check, which is the whole of what this build knows about a proxy target.
    #[test]
    fn an_upstream_is_an_absolute_http_address_with_a_host() {
        for good in [
            "http://127.0.0.1:8080",
            "https://localhost:5173",
            "http://127.0.0.1:3000/api",
        ] {
            upstream_is_an_address(good).expect(good);
        }

        for bad in [
            "127.0.0.1:8080",
            "ftp://127.0.0.1",
            "http://",
            "http://127.0.0.1:8080?a=b",
            "http://127.0.0.1:8080#top",
        ] {
            assert!(upstream_is_an_address(bad).is_err(), "{bad} was accepted");
        }
    }

    /// A doc root joins onto the root with whatever separator this OS uses, and `""` is the root.
    #[test]
    fn a_stored_doc_root_joins_back_onto_its_root() {
        let root = Path::new("/srv/blog");

        assert_eq!(doc_root_full(root, ""), PathBuf::from("/srv/blog"));
        assert_eq!(
            doc_root_full(root, "public"),
            PathBuf::from("/srv/blog").join("public")
        );
        assert_eq!(
            doc_root_full(root, "web/public"),
            PathBuf::from("/srv/blog").join("web").join("public")
        );
    }
}
