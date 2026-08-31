//! Turning declared state into configuration on disk, and into something the supervisor can run.
//!
//! Roadmap task **T30**. Two halves of one job, which is why they are one module:
//!
//! - **Config generation.** Every managed service's configuration is rendered from a template plus
//!   the user's overrides into `etc/<service-id>/`, atomically, skipping what has not changed and
//!   installing nothing that fails validation. That is [`document`] and [`settings`].
//! - **The `ServiceSpec` in front of it.** A `services` row is not a spec — it carries
//!   `package_id`, `port`, `data_dir`, `config_overrides_json` and `limits_json` — and what turns
//!   one into a runnable specification is the same knowledge that renders its config: a [`recipe`].
//!
//! So this module is also the answer to the port `mixengine-daemon` has been asking through since
//! T19 (`SpecSource`), and the thing that answers it is [`Generator`]. Before this, the daemon's
//! shipped source was `Undeclared` — an empty set, honest for a build that could not render a spec
//! at all.
//!
//! # What is generated is never read back
//!
//! `CLAUDE.md`'s rule, and this module is where it is kept: nothing here parses a file under `etc/`
//! into state. The only read is [`document::install`] comparing a rendering against what is on disk
//! to decide whether to write it, which is a *checksum* rather than a parse — it can tell you the
//! file is the one we would write, and it can tell you nothing about a file that is not.
//!
//! # Rendered on every walk
//!
//! [`Generator::declared`] renders and installs, and the registry asks it at the top of every
//! `service.*` call. That sounds like a lot of writing and is almost none: the diff in
//! [`document::install`] means a home whose state has not changed does no I/O beyond reading each
//! file once. The alternative — rendering only when something is started — leaves the two able to
//! disagree, and the way that is discovered is a service reloading a config from before the change
//! the user just made.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use mixengine_platform::PortBinding;
use mixengine_proto::{IdlePolicy, Millis, ResourceLimits, ServiceId, ServiceSpec};

pub mod document;
pub mod first_run;
pub mod recipe;
pub mod recipes;
pub mod served;
pub mod settings;

pub use document::{Document, Reason, Validator, Written};
pub use first_run::{DataDirectory, FirstRun, Ritual, SecretSpec, Step};
pub use recipe::{
    Catalogue, Context, Endpoints, Instancing, Recipe, Role, Source, TemplateFile, Upstream,
    Upstreams,
};
pub use recipes::{Caddy, Mariadb, PhpFpm, Postgres};
pub use served::{Served, ServedKind, Shared};
pub use settings::{Preset, Setting, Settings, Value};

use crate::{Error, Paths, Result, Store};

/// The `services` table, rendered.
///
/// Holds the home's layout, its database and the recipes this build can find — everything needed to
/// answer "what does this home declare" with no argument at all, which is what the daemon's
/// `SpecSource` is asked.
#[derive(Debug, Clone)]
pub struct Generator {
    /// Where `etc/`, `data/`, `run/` and `logs/` are.
    paths: Paths,

    /// The declared state.
    store: Store,

    /// What this build knows how to run.
    catalogue: Catalogue,

    /// What this system makes a program bind to answer on the ports a front end serves.
    ///
    /// Asked once, when the generator is built, because the mapping is a constant of the operating
    /// system — `PortAccess::bindings` is pure for exactly this reason.
    bindings: Vec<PortBinding>,
}

/// One service, generated: what it will run, and what changed on the way.
#[derive(Debug, Clone)]
pub struct Generated {
    /// What the supervisor is to run.
    pub spec: ServiceSpec,

    /// Every file the recipe renders, and what installing it did.
    ///
    /// What a reload decision is made of, which is the reason it is reported at all — see
    /// [`Written::changed`].
    pub files: Vec<(PathBuf, Written)>,

    /// Files a swept directory carried that no document of this service's owns any more.
    ///
    /// Counted by [`Generated::changed`], and that is the whole reason it is reported: a walk whose
    /// only difference is a deleted site has to reach the reload, or the front end goes on serving
    /// a site nothing declares.
    pub removed: Vec<PathBuf>,

    /// What has to happen once before this service is ever started, if anything.
    ///
    /// Computed here because this is the only place both halves are in hand — the recipe, and a
    /// [`Context`] built from the row. Assembling it costs a clone of that context and nothing else;
    /// it is *performed* by the daemon, once, and only when the markers say it has not been.
    pub first_run: Option<FirstRun>,
}

impl Generated {
    /// Whether anything on disk is different from what it was.
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.removed.is_empty() || self.files.iter().any(|(_, written)| written.changed())
    }
}

/// One row, resolved as far as it can be before anything else's address is known.
///
/// The generator's first pass builds one of these per row. It exists because of one dependency: a
/// site's `fastcgi_pass` names where its pool listens, and the pool's own recipe is the only thing
/// that knows — so every context has to exist before the first file is rendered.
#[derive(Debug)]
struct Prepared {
    recipe: Arc<dyn Recipe>,
    context: Context,
    limits: ResourceLimits,
    /// How long this service may look idle before it is stopped, or [`None`] for never.
    ///
    /// Already resolved against the recipe's default and the column's three states — see
    /// [`Generator::prepare`]. Kept as a duration rather than as a whole `IdlePolicy` because the
    /// other half of one is the probe, and a probe may name an endpoint that is not computed until
    /// after this struct is built.
    idle_after: Option<Millis>,
}

/// One `services` row joined to **both** of the tables a parent could be in.
///
/// A struct rather than the query's own anonymous record, because two call sites read it and
/// `sqlx::query!` gives each of them a different type.
///
/// Seven nullable columns rather than three, because the join that did not match contributes nulls
/// and the `CHECK` on `services` guarantees exactly one of the two groups is whole. Resolving that
/// into one answer is [`Parent::of`], which is also where a row that matched neither is refused.
#[derive(Debug)]
struct Row {
    id: String,
    instance_name: String,
    port: Option<i64>,
    bind_addr: String,
    data_dir: Option<String>,
    overrides: String,
    limits: String,
    /// `NULL` is "use the recipe's default", `0` is "never", and `n` is minutes.
    idle_minutes: Option<i64>,
    /// The port the activator listens on, for a service that listens on TCP and can be started by a
    /// connection. `NULL` for every other row, which is most of them — T70.
    activation_port: Option<i64>,
    package: Option<String>,
    package_version: Option<String>,
    package_path: Option<String>,
    package_provides: Option<String>,
    runtime: Option<String>,
    runtime_version: Option<String>,
    runtime_path: Option<String>,
    runtime_provides: Option<String>,
}

/// One service, and what would change on disk if it were rendered now.
///
/// The answer [`Generator::drift`] gives per row. A service whose [`drift`](Self::drift) is empty is
/// one whose installed configuration is already what its row renders to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDrift {
    /// Which service.
    pub service: ServiceId,

    /// What would change.
    pub drift: document::Drift,
}

/// Which install supplies the binary behind one service, resolved from the two halves of a [`Row`].
///
/// `Parent` and not `Origin`: the public [`services::Origin`](crate::services::Origin) is what a
/// caller *asks* for and this is what a row *has*, and `recipe.rs` already has a private `Origin` of
/// its own for the `package` half of a rendering.
#[derive(Debug)]
struct Parent {
    /// The name the recipe is found under, which is also what `data/<package>` is named after.
    package: String,

    /// The installed version, as upstream writes it.
    version: String,

    /// Where that install is unpacked.
    install_path: String,

    /// What it calls its executables, and where each one is inside the directory.
    provides: BTreeMap<String, String>,
}

impl Parent {
    /// Read a row's parent, whichever of the two it has.
    ///
    /// **The recipe's name for a runtime is the id's own half**, and that is the one asymmetry worth
    /// stating: a `packages` row names itself `caddy` and the service is `caddy`, while a
    /// `runtime_installs` row names itself `php` and the service is `php-fpm@8.3.33`. What finds a
    /// recipe is `ServiceId::name()` either way — the rule `recipe.rs` already states — so a pool
    /// takes its name from the id and the runtime's kind stops here.
    ///
    /// **A package publishes a `provides` map now, and a row written before migration 0004 carries
    /// an empty one** — which is honest rather than a placeholder. A Caddy installed before that
    /// column existed is served by [`Context::program`], which asks this map nothing. What an empty
    /// map costs is a recipe that does ask — MariaDB, whose seven commands are not one binary at the
    /// install root — and the answer it gets names the reinstall that would fill it in. See
    /// [`Context::provided`].
    fn of(row: &mut Row, service: &ServiceId) -> Result<Self> {
        let unreadable = |value: &str| Error::UnreadableServiceRow {
            service: service.as_str().to_owned(),
            column: "package_id",
            value: value.to_owned(),
        };

        match (
            row.package.take(),
            row.package_version.take(),
            row.package_path.take(),
            row.package_provides.take(),
        ) {
            (Some(package), Some(version), Some(install_path), Some(provides)) => {
                let provides = serde_json::from_str(&provides).map_err(|source| {
                    Error::UnreadableServiceDocument {
                        service: service.as_str().to_owned(),
                        column: "provides_json",
                        source,
                    }
                })?;

                return Ok(Self {
                    package,
                    version,
                    install_path,
                    provides,
                });
            }
            (None, None, None, None) => {}
            _ => return Err(unreadable("a packages row that is only half there")),
        }

        match (
            row.runtime.take(),
            row.runtime_version.take(),
            row.runtime_path.take(),
            row.runtime_provides.take(),
        ) {
            (Some(_kind), Some(version), Some(install_path), Some(provides)) => {
                let provides = serde_json::from_str(&provides).map_err(|source| {
                    Error::UnreadableServiceDocument {
                        service: service.as_str().to_owned(),
                        column: "provides_json",
                        source,
                    }
                })?;

                Ok(Self {
                    package: service.name().to_owned(),
                    version,
                    install_path,
                    provides,
                })
            }

            // The `CHECK` on `services` makes this unreachable through the database's own rules, so
            // reaching it means a row somebody wrote by hand or a runtime removed out from under
            // one. Named rather than defaulted, because a service silently rendered against no
            // install is a service that fails much later and somewhere else.
            _ => Err(unreadable("neither a package nor a runtime install")),
        }
    }
}

impl Generator {
    /// A generator for this home.
    ///
    /// `bindings` is what the platform layer says this system makes a program bind to answer on 80
    /// and 443 — [`PortAccess::bindings`](mixengine_platform::PortAccess::bindings). It is a value
    /// rather than a call because `mixengine-core` may not ask what system it is on, and because a
    /// generator is built once and renders on every walk.
    #[must_use]
    pub fn new(
        paths: Paths,
        store: Store,
        catalogue: Catalogue,
        bindings: Vec<PortBinding>,
    ) -> Self {
        Self {
            paths,
            store,
            catalogue,
            bindings,
        }
    }

    /// What this home declares, prepared but not yet written anywhere.
    ///
    /// **The step both [`declared`](Self::declared) and [`drift`](Self::drift) start from**, and the
    /// reason it is one function: the two ask the same question of the same rows and differ only in
    /// what they do with the answer. Two walks would be two definitions of "what this home
    /// declares", and the front end — whose configuration is a function of every site and of where
    /// every pool listens — is where they would come apart.
    ///
    /// **All or nothing.** One row that cannot be prepared fails the call rather than being left out
    /// of the answer, because a service that silently disappears from `mix service list` is a service
    /// somebody goes looking for in the wrong place. What they get instead is a message naming the
    /// row and what is wrong with it.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the rows cannot be read; and per row, whatever
    /// [`generate`](Self::generate) reports.
    async fn declarations(&self) -> Result<(Vec<Prepared>, Vec<Served>)> {
        let rows = sqlx::query_as!(
            Row,
            r#"SELECT s.id                    AS "id!: String",
                      s.instance_name         AS "instance_name!: String",
                      s.port                  AS "port: i64",
                      s.bind_addr             AS "bind_addr!: String",
                      s.data_dir              AS "data_dir: String",
                      s.config_overrides_json AS "overrides!: String",
                      s.limits_json           AS "limits!: String",
                      s.idle_minutes          AS "idle_minutes: i64",
                      s.activation_port       AS "activation_port: i64",
                      p.name                  AS "package: String",
                      p.version               AS "package_version: String",
                      p.install_path          AS "package_path: String",
                      p.provides_json         AS "package_provides: String",
                      r.kind                  AS "runtime: String",
                      r.version               AS "runtime_version: String",
                      r.install_path          AS "runtime_path: String",
                      r.provides_json         AS "runtime_provides: String"
               FROM services s
               LEFT JOIN packages p         ON p.id = s.package_id
               LEFT JOIN runtime_installs r ON r.id = s.runtime_install_id
               ORDER BY s.id"#
        )
        .fetch_all(self.store.pool())
        .await
        .map_err(|source| self.store.failure("read", source))?;

        // **Two passes, and the first one writes nothing.** A site's configuration names the address
        // its pool listens on, and only the pool's own recipe knows what that is — so every context
        // has to be built before any file is rendered. The pass costs a `Context` per row out of a
        // row already fetched.
        let mut prepared = Vec::with_capacity(rows.len());

        for row in rows {
            prepared.push(self.prepare(row)?);
        }

        let mut upstreams = BTreeMap::new();

        for one in &prepared {
            if let Some(listen) = one.recipe.upstream(&one.context)? {
                // Asked only of a service something points at, so a recipe with no upstream is never
                // asked for an activator either — T70. The pair is inserted together, which is what
                // makes it impossible for a site to name one service's pool and another's activator.
                let activator = one.recipe.activator(&one.context)?;

                upstreams.insert(one.context.service.clone(), Upstreams { listen, activator });
            }
        }

        let served = served::served(&self.store, &upstreams, self.paths.certs()).await?;

        Ok((prepared, served))
    }

    /// Every service this home declares, configured and ready to run.
    ///
    /// **The whole set**, because the caller's next move is a
    /// [`ServiceGraph`](crate::services::ServiceGraph): dependencies, cycles and start order are
    /// properties of a set.
    ///
    /// **[`Generated`] and not [`ServiceSpec`]**, because the specification is only half of what a
    /// walk needs to know. The other half is whether anything on disk moved — [`Generated::changed`]
    /// — which is what turns "the configuration is up to date" into "the process reading it has been
    /// told", and it is knowledge only this call has: by the time a spec is in a caller's hands the
    /// file has already been written and a second look at it would compare a rendering with itself.
    ///
    /// [`drift`](Self::drift) is the read-only half, and answers what this would change.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the rows cannot be read; per row, whatever
    /// [`generate`](Self::generate) reports; and per row, whatever installing it reports.
    pub async fn declared(&self) -> Result<Vec<Generated>> {
        let (prepared, served) = self.declarations().await?;
        let mut generated = Vec::with_capacity(prepared.len());

        for one in prepared {
            generated.push(self.install(one, &served).await?);
        }

        Ok(generated)
    }

    /// One service's settings, merged, without rendering or installing anything.
    ///
    /// **Read-only, and that is the whole reason it exists** — roadmap task **T53**.
    /// [`generate`](Self::generate) goes through [`declared`](Self::declared), which installs; a
    /// caller that only wants a number would rewrite this home's configuration to get it, and can
    /// reload a running server as a side effect of asking a question. This takes
    /// [`drift`](Self::drift)'s door for the reason stated there — rendering is pure, and nothing
    /// before it touches a disk.
    ///
    /// **The same merge and not a second one.** What comes back is what a rendering of this service
    /// would be made with, so a caller and a template cannot come to disagree about a value. The
    /// first caller is `mix cert status`, which connects to the front end's `https_port`; the only
    /// other way to learn that number is to read it out of the generated configuration, and
    /// `.claude/CLAUDE.md` forbids parsing a generated file back into state.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the rows cannot be read, whatever merging the overrides reports,
    /// and [`Error::NotFound`] when nothing in this home declares this service.
    pub async fn settings(&self, service: &ServiceId) -> Result<Settings> {
        let (prepared, _served) = self.declarations().await?;

        prepared
            .into_iter()
            .find(|one| one.context.service() == service)
            .map(|one| one.context.settings().clone())
            .ok_or_else(|| Error::NotFound {
                kind: "service",
                id: service.as_str().to_owned(),
            })
    }

    /// What [`declared`](Self::declared) would change on disk, asked without changing it.
    ///
    /// Roadmap task **T47b**, and the read `mix doctor`'s tenth check makes. Rendering is pure — it
    /// builds [`Document`]s in memory — and the comparison is [`document::drift`], so nothing is
    /// staged, nothing is validated and no directory is created. That is what lets a check whose
    /// whole guarantee is "this writes nothing" ask the question at all.
    ///
    /// **A home with no rows drifts in no way**, and answers an empty list rather than an error.
    ///
    /// # Errors
    ///
    /// [`Error::Database`] when the rows cannot be read; per row, whatever
    /// [`generate`](Self::generate) reports; and [`Error::Io`] when a generated directory cannot
    /// be read.
    pub async fn drift(&self) -> Result<Vec<ServiceDrift>> {
        let (prepared, served) = self.declarations().await?;
        let mut drifts = Vec::with_capacity(prepared.len());

        for one in &prepared {
            drifts.push(ServiceDrift {
                service: one.context.service.clone(),
                drift: document::drift(
                    &one.context.etc,
                    &Self::documents(one, &served)?,
                    one.recipe.swept(),
                )
                .await?,
            });
        }

        Ok(drifts)
    }

    /// Every service something can start by connecting to it, and the pair of addresses that takes
    /// — roadmap task **T70**.
    ///
    /// The activator's address first and the service's own second, which is the order they are used
    /// in: the daemon holds the first and dials the second once the service is up. A service whose
    /// recipe has no activator is not in the map at all.
    ///
    /// **Computed from the same rows and the same contexts a render uses**, so the address the
    /// daemon binds and the address a site file names cannot disagree — which is the failure this
    /// method exists to make impossible, because what it looks like is a site that 502s and an
    /// activator sitting on an address nothing dials.
    ///
    /// # Errors
    ///
    /// Whatever preparing a row costs, and whatever computing either address costs — a home too
    /// deeply nested for the derived socket path, most of all.
    pub async fn activators(&self) -> Result<BTreeMap<ServiceId, (Upstream, Upstream)>> {
        // Through `declarations` rather than a query of its own: the addresses have to be the ones
        // a render would compute, and a second row query here would be a second chance to compute
        // them from something slightly different.
        let (prepared, _served) = self.declarations().await?;

        let mut activators = BTreeMap::new();

        for one in prepared {
            let Some(listen) = one.recipe.upstream(&one.context)? else {
                continue;
            };

            if let Some(activator) = one.recipe.activator(&one.context)? {
                activators.insert(one.context.service.clone(), (activator, listen));
            }
        }

        Ok(activators)
    }

    /// Every address in this home that a connection may start a *stopped* service at — **T70a**.
    ///
    /// **Through `declarations` for [`activators`](Self::activators)' reason**: the addresses have
    /// to be the ones a render computed, and a second query here would be a second chance to
    /// compute them from something slightly different — which is exactly what would happen to a
    /// socket path, whose value is a function of how deep this home is.
    ///
    /// **A service with nothing to hold is absent rather than present and empty.** The caller
    /// iterates this map, and an empty entry is a service it would log about having done nothing
    /// for.
    ///
    /// # Errors
    ///
    /// Whatever preparing every service's context costs, and whatever a recipe reports about an
    /// address it cannot compute.
    pub async fn held_while_stopped(&self) -> Result<BTreeMap<ServiceId, Vec<Upstream>>> {
        let (prepared, _served) = self.declarations().await?;

        let mut held = BTreeMap::new();

        for one in prepared {
            let addresses = one.recipe.held_while_stopped(&one.context)?;

            if !addresses.is_empty() {
                held.insert(one.context.service.clone(), addresses);
            }
        }

        Ok(held)
    }

    /// Every file this service has, rendered.
    ///
    /// **One definition for both callers.** [`install`](Self::install) writes these and
    /// [`drift`](Self::drift) compares them; a second assembly of the set would be a second answer
    /// to "what does this service render to", and the front end's site files are exactly where two
    /// answers would diverge.
    fn documents(prepared: &Prepared, served: &[Served]) -> Result<Vec<Document>> {
        let mut documents = recipe::render(prepared.recipe.as_ref(), &prepared.context)?;

        // **Appended to the recipe's own set, not installed by a path of its own** — T43's D1. The
        // checker judges a staging directory, so a site file written anywhere else would be
        // invisible to `caddy validate` and present at run time.
        if prepared.recipe.role() == Role::FrontEnd {
            documents.extend(prepared.recipe.sites(&prepared.context, served)?);
        }

        Ok(documents)
    }

    /// Generate one service's configuration and specification.
    ///
    /// **The whole home is rendered and one answer is picked out**, which is not a shortcut: a front
    /// end's configuration is a function of every site in the database and of where every pool
    /// listens, so "render this one service" is not a smaller job than rendering all of them. It was
    /// one before T43, and pretending it still is would mean a second, subtly different assembly of
    /// the same map.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is no such service, and everything
    /// [`declared`](Self::declared) can report.
    pub async fn generate(&self, service: &ServiceId) -> Result<Generated> {
        self.declared()
            .await?
            .into_iter()
            .find(|one| one.spec.id() == service)
            .ok_or_else(|| Error::NotFound {
                kind: "service",
                id: service.as_str().to_owned(),
            })
    }

    /// One row, as far as it can be taken before anything else's address is known: look up the
    /// recipe, merge the overrides, build the context.
    ///
    /// Nothing here writes, which is what makes a first pass over every row affordable.
    fn prepare(&self, mut row: Row) -> Result<Prepared> {
        let service =
            ServiceId::parse(row.id.clone()).map_err(|source| Error::UnreadableServiceRow {
                service: row.id.clone(),
                column: "id",
                value: source.to_string(),
            })?;

        let parent = Parent::of(&mut row, &service)?;

        let recipe = self
            .catalogue
            .recipe(&parent.package)
            .ok_or_else(|| Error::NoRecipe {
                service: row.id.clone(),
                package: parent.package.clone(),
                known: self.catalogue.packages().map(str::to_owned).collect(),
            })?
            .clone();

        let port = match row.port {
            None => None,
            Some(port) => Some(
                u16::try_from(port).map_err(|_| Error::UnreadableServiceRow {
                    service: row.id.clone(),
                    column: "port",
                    value: port.to_string(),
                })?,
            ),
        };

        // The activator's own port — roadmap task T70. Read exactly as `port` above is, and
        // separate from it for the reason the column is separate: it is allocated, not derived, so
        // there is nothing here to compute it from.
        let activation_port = match row.activation_port {
            None => None,
            Some(port) => Some(
                u16::try_from(port).map_err(|_| Error::UnreadableServiceRow {
                    service: row.id.clone(),
                    column: "activation_port",
                    value: port.to_string(),
                })?,
            ),
        };

        let settings = Settings::merge(recipe.settings(), &row.overrides, &service)?;

        let limits: ResourceLimits = serde_json::from_str(&row.limits).map_err(|source| {
            Error::UnreadableServiceDocument {
                service: row.id.clone(),
                column: "limits_json",
                source,
            }
        })?;

        // **The row's half of an idle policy, and its three states — roadmap task T69.** A policy
        // cannot be assembled here: the other half is the recipe's probe, and a probe may name an
        // endpoint, which is computed a few lines below this. What is decided here is only whether
        // there is a policy at all and how long it waits.
        let idle_after = match row.idle_minutes {
            // Nobody has said. Whatever the recipe wants — which since T70 is half an hour for a
            // php-fpm pool and nothing for everything else, until T70a can start a database again.
            None => recipe.idle_default(),

            // Said, and said no. Outranks the recipe deliberately: a default arriving in a later
            // release must not switch idle-stopping back on behind the person who turned it off.
            Some(0) => None,

            Some(minutes) => {
                let minutes = u64::try_from(minutes).map_err(|_| Error::UnreadableServiceRow {
                    service: row.id.clone(),
                    column: "idle_minutes",
                    value: minutes.to_string(),
                })?;

                Some(Millis::from_secs(minutes.saturating_mul(60)))
            }
        };

        let mut context = Context {
            etc: self.paths.etc().join(service.as_str()),
            etc_root: self.paths.etc().to_path_buf(),

            // The row wins, and the fallback is the package's rather than `data/<service-id>`: two
            // instances of one server are `mariadb@main` and `mariadb@legacy`, and a directory named
            // after the *package* is what makes them siblings in a listing rather than two unrelated
            // names. How far down that goes is the recipe's answer, not this function's.
            data: row.data_dir.map_or_else(
                || match recipe.instancing() {
                    // A server that exists once has no instance half to spend, and `data/caddy/caddy`
                    // reads as a mistake to whoever finds it.
                    Instancing::Single => self.paths.data().join(&parent.package),
                    Instancing::Named => self
                        .paths
                        .data()
                        .join(&parent.package)
                        .join(&row.instance_name),
                },
                PathBuf::from,
            ),
            run: self.paths.run().to_path_buf(),
            logs: self.paths.service_logs(&service),
            package: parent.package,
            version: parent.version,
            install_path: PathBuf::from(parent.install_path),
            provides: parent.provides,
            port,
            activation_port,
            bind: row.bind_addr,
            settings,
            endpoints: recipe::Endpoints::default(),
            bindings: self.bindings.clone(),

            // **This home's authority, for a front end to hand to a phone** — roadmap task T75.
            // Read here rather than in a recipe, on the rule `bindings` follows: a recipe is a
            // function of its context, and a recipe that went looking at a disk would be a second
            // place this path is spelled. A home with no authority renders no copy of one.
            authority: std::fs::read_to_string(crate::certs::ca::certificate_path(
                self.paths.certs(),
            ))
            .ok(),
            secrets: BTreeMap::new(),
            service,
        };

        // Asked once and stored, rather than recomputed by the template and again by the spec: the
        // whole point is that there is one answer. Before the render, because the template reads it.
        context.endpoints = recipe.endpoints(&context)?;

        Ok(Prepared {
            recipe,
            context,
            limits,
            idle_after,
        })
    }

    /// A prepared row, all the way to a spec: render, add the sites if it is the front end, install,
    /// build.
    ///
    /// The order is forced and each step depends on the last. Installing before building the spec
    /// is the one that could be argued: a spec that will not build leaves a configuration on disk
    /// for a service that cannot start. That is the right way round — the config is what a person
    /// reads to work out *why* it will not start, and a spec that does not build is a bug in a
    /// recipe rather than a state anybody has to recover from.
    async fn install(&self, prepared: Prepared, served: &[Served]) -> Result<Generated> {
        // Rendered before the row is taken apart, and through the helper `drift` uses, so the set
        // that gets installed is the set that gets compared. The role is what selects the recipe, on
        // T37's rule — a home has at most one front end, because `service.create` refuses a second.
        let documents = Self::documents(&prepared, served)?;

        let Prepared {
            recipe,
            context,
            limits,
            idle_after,
        } = prepared;

        // Before the render is judged, because a validator judges a *running* configuration and a
        // running configuration names places. php-fpm opens its `error_log` during `--test` and
        // fails the whole file when the directory is not there — and the service log directory is
        // otherwise created by the log sink, which is to say at the first start, which is after
        // this. The supervisor still creates it: this is the earlier of two idempotent calls, not a
        // move of the responsibility.
        crate::paths::create_dir(&context.logs)?;

        // **And the data directory, for the same reason one step out.** A server that names its own
        // data directory in its own configuration does not create it: Redis reads `dir` and refuses
        // the whole file with `FATAL CONFIG FILE ERROR … No such file or directory`, and a service
        // whose working directory is missing does not even reach its first line — memcached fails to
        // spawn. The two recipes that came before them hid this, because a first-run ritual creates
        // the directory it is about to bootstrap into, and Caddy makes its own storage.
        //
        // Creating it empty changes nothing for those rituals: `first_run::inspect` answers
        // [`DataDirectory::Empty`] for a directory that is missing *and* for one that is there with
        // nothing in it, and an empty datadir is the one thing Windows' `mariadb-install-db` accepts.
        //
        // [`DataDirectory::Empty`]: first_run::DataDirectory::Empty
        crate::paths::create_dir(&context.data)?;

        let installed = document::install(
            &context.etc,
            &documents,
            recipe.swept(),
            recipe.validator(&context).as_ref(),
        )
        .await?;

        let files = documents
            .iter()
            .map(|document| document.relative().to_path_buf())
            .zip(installed.written)
            .collect();

        let mut builder = recipe
            .spec(&context)?
            .limits(limits)
            // **Off the recipe and never off a row** — roadmap task T71a. Whether this program
            // survives a restart under memory pressure is a fact about the program, so it is carried
            // onto every spec the recipe renders rather than joined from anything per-home.
            .restart_over_memory(recipe.restart_over_memory_default());

        // **Both halves or neither.** A probe with no duration is a measurement nobody acts on, and
        // a duration with no probe is the wall clock an `IdlePolicy` exists to not be — so a recipe
        // that declares no probe is never idle-stopped however its row is set.
        if let (Some(after), Some(probe)) = (idle_after, recipe.idle_probe(&context)) {
            builder = builder.idle(IdlePolicy { after, probe });
        }

        let spec = builder.build().map_err(|source| Error::Unrunnable {
            service: context.service.as_str().to_owned(),
            source,
        })?;

        let first_run = recipe
            .ritual()
            .map(|ritual| FirstRun::new(&context, ritual));

        Ok(Generated {
            spec,
            files,
            removed: installed.removed,
            first_run,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mixengine_proto::{Millis, ReadyCheck, RestartPolicy, ServiceSpecBuilder, StopBehaviour};
    use mixengine_testkit::FakeService;

    use super::*;
    use crate::config::PathOverrides;

    /// A recipe over `fakeservice`, with one file and one setting.
    ///
    /// Deliberately not the daemon's own fixture recipe: this one is about the *generator*, so it
    /// renders something whose content is worth asserting and takes a setting worth overriding.
    #[derive(Debug)]
    struct Fake;

    const GREETING: &str = "greeting";

    impl Recipe for Fake {
        fn package(&self) -> &'static str {
            "fakeservice"
        }

        /// Set so that the generator carrying it onto the spec is observable — task **T71a**.
        ///
        /// The trait's default is `false`, so a fixture that took it would prove nothing about
        /// whether `generate` reads this at all.
        fn restart_over_memory_default(&self) -> bool {
            true
        }

        fn instancing(&self) -> Instancing {
            Instancing::Named
        }

        fn settings(&self) -> &'static [Setting] {
            &[Setting {
                key: GREETING,
                default: Preset::Text("hello"),
            }]
        }

        fn files(&self) -> &'static [TemplateFile] {
            &[TemplateFile {
                path: "fakeservice.conf",
                source: "say = {{ settings.greeting }}\nport = {{ service.port }}\n{{ extra }}",
            }]
        }

        /// Measured on the port the row allocated, as every server recipe here is.
        ///
        /// Its sibling `idle_default` is left at the trait's `None`, which is what every shipped
        /// recipe answers too — so this fixture exercises the join without pretending a default
        /// exists anywhere.
        fn idle_probe(&self, context: &Context) -> Option<mixengine_proto::IdleProbe> {
            context
                .port()
                .map(|port| mixengine_proto::IdleProbe::Connections { port })
        }

        /// On the port the row allocated, as every database recipe answers — T70a.
        ///
        /// A fixture rather than a real recipe, because what the generator is asked here is
        /// whether it carries a recipe's answer out of the same pass that renders. *Which*
        /// addresses each database names is its own recipe's test.
        fn held_while_stopped(&self, context: &Context) -> Result<Vec<Upstream>> {
            Ok(context
                .port()
                .map(|port| {
                    vec![Upstream::Tcp(std::net::SocketAddr::from((
                        std::net::Ipv4Addr::LOCALHOST,
                        port,
                    )))]
                })
                .unwrap_or_default())
        }

        fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
            Ok(
                ServiceSpec::builder(context.service().clone(), FakeService::program())
                    .cwd(context.data())
                    .ready(ReadyCheck::PidAlive { settle: Millis(10) })
                    .restart(RestartPolicy::Never)
                    .stop(StopBehaviour::Signal { grace: Millis(500) }),
            )
        }
    }

    /// A recipe for a package that exists once, so its id carries no `@`.
    ///
    /// Nothing but the instancing and the working directory: what it is here to show is where the
    /// generator puts a singleton's data, and a template would only be noise around that.
    #[derive(Debug)]
    struct Solo;

    impl Recipe for Solo {
        fn package(&self) -> &'static str {
            "solo"
        }

        fn instancing(&self) -> Instancing {
            Instancing::Single
        }

        fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
            Ok(
                ServiceSpec::builder(context.service().clone(), FakeService::program())
                    .cwd(context.data())
                    .ready(ReadyCheck::PidAlive { settle: Millis(10) })
                    .restart(RestartPolicy::Never)
                    .stop(StopBehaviour::Signal { grace: Millis(500) }),
            )
        }
    }

    /// A recipe that is a front end, so the generator asks it for sites.
    ///
    /// It renders one file per site with the domain in it, and nothing else: what is under test here
    /// is the *plumbing* — that a front end is asked, that the sites it is given are the enabled ones
    /// with their upstreams resolved, and that what it returns joins the set the validator judges.
    /// What a real Caddyfile or `server` block has to say is judged by the real server, in
    /// `crates/mixengine-cli/tests/{caddy,nginx}.rs`.
    #[derive(Debug)]
    struct Front;

    impl Recipe for Front {
        fn package(&self) -> &'static str {
            "front"
        }

        fn instancing(&self) -> Instancing {
            Instancing::Single
        }

        fn role(&self) -> Role {
            Role::FrontEnd
        }

        fn swept(&self) -> &'static [&'static str] {
            &["sites"]
        }

        fn sites(&self, _context: &Context, served: &[Served]) -> Result<Vec<Document>> {
            Ok(served
                .iter()
                .map(|site| {
                    Document::new(
                        format!("sites/{}.conf", site.primary()),
                        format!(
                            "serve {} from {}\n",
                            site.primary(),
                            site.doc_root.display()
                        ),
                    )
                })
                .collect())
        }

        fn spec(&self, context: &Context) -> Result<ServiceSpecBuilder> {
            Ok(
                ServiceSpec::builder(context.service().clone(), FakeService::program())
                    .cwd(context.data())
                    .ready(ReadyCheck::PidAlive { settle: Millis(10) })
                    .restart(RestartPolicy::Never)
                    .stop(StopBehaviour::Signal { grace: Millis(500) }),
            )
        }
    }

    /// D1 and D4 together: a site is rendered into the front end's own set, and a site that stops
    /// being declared takes its file with it — on the same walk, so the same reload carries both.
    #[tokio::test]
    async fn a_front_end_renders_the_sites_this_home_declares_and_sweeps_the_ones_it_does_not() {
        let (home, generator) = home_of(Arc::new(Front), "front", "front", "{}").await;

        sqlx::query(
            "INSERT INTO projects (id, name, root_path, created_at)
             VALUES (1, 'blog', '/src/blog', '2026-08-23T00:00:00Z')",
        )
        .execute(generator.store.pool())
        .await
        .expect("a project");

        sqlx::query(
            "INSERT INTO sites (id, project_id, doc_root, kind, state)
             VALUES (1, 1, 'public', 'static', 'enabled')",
        )
        .execute(generator.store.pool())
        .await
        .expect("a site");

        sqlx::query(
            "INSERT INTO site_domains (site_id, domain, is_primary) VALUES (1, 'blog.test', 1)",
        )
        .execute(generator.store.pool())
        .await
        .expect("a domain");

        let first = generator.declared().await.expect("a rendering");
        assert!(first[0].changed());

        let sites = home.path().join("etc").join("front").join("sites");
        assert!(
            sites.join("blog.test.conf").is_file(),
            "the site was not rendered"
        );

        // Nothing moved, so nothing is written and nothing reloads. This is what "idempotent
        // re-runs" is: a property of the diff rather than a feature anybody implemented.
        let again = generator.declared().await.expect("a second rendering");
        assert!(!again[0].changed(), "an unchanged home wrote something");

        sqlx::query("UPDATE sites SET state = 'disabled' WHERE id = 1")
            .execute(generator.store.pool())
            .await
            .expect("the site is turned off");

        let after = generator.declared().await.expect("a third rendering");

        assert!(
            !sites.join("blog.test.conf").exists(),
            "a disabled site's file survived, so it is still being served"
        );
        assert!(
            after[0].changed(),
            "the removal did not reach the reload, so the front end went on serving it"
        );
    }

    /// **The daemon is told where to hold from the same pass that renders** — T70a.
    ///
    /// Never from a query of its own, which is why `activators` goes through `declarations` and
    /// why this does too: an address computed twice is an address that can be computed two ways,
    /// and the two would diverge on exactly the recipe whose socket path depends on how deep this
    /// home is.
    #[tokio::test]
    async fn a_service_reports_the_addresses_it_is_woken_at() {
        let (_home, generator) = home("{}").await;

        let held = generator
            .held_while_stopped()
            .await
            .expect("the addresses this home is woken at");

        assert_eq!(
            held.get(&ServiceId::parse("fakeservice@main").expect("an id")),
            Some(&vec![Upstream::Tcp(std::net::SocketAddr::from((
                std::net::Ipv4Addr::LOCALHOST,
                4321
            )))]),
            "the recipe's answer did not reach the caller: {held:?}"
        );
    }

    /// A home with a database, a package row and one service row for `recipe`'s package.
    ///
    /// `instance` is the `instance_name` column rather than something derived here, because what a
    /// row puts in it is exactly what these tests are about: an instance of a named package writes
    /// the half after the `@`, and a package that exists once writes its own name.
    async fn home_of(
        recipe: Arc<dyn Recipe>,
        id: &str,
        instance: &str,
        overrides: &str,
    ) -> (tempfile::TempDir, Generator) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(directory.path().to_path_buf(), &PathOverrides::default());
        let store = Store::open(paths.database_file())
            .await
            .expect("a database");
        let package = recipe.package();

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-08-15T00:00:00Z', 'https://example', 'ab')",
        )
        .bind(package)
        .execute(store.pool())
        .await
        .expect("a package row");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state, port,
                                   config_overrides_json)
             VALUES (?, (SELECT id FROM packages WHERE name = ?), ?, 'stopped', 4321, ?)",
        )
        .bind(id)
        .bind(package)
        .bind(instance)
        .bind(overrides)
        .execute(store.pool())
        .await
        .expect("a services row");

        let catalogue = Catalogue::builtin().with(recipe);

        (
            directory,
            Generator::new(paths, store, catalogue, Vec::new()),
        )
    }

    /// A home holding one `fakeservice@main`, which is what most of these tests want.
    async fn home(overrides: &str) -> (tempfile::TempDir, Generator) {
        home_of(Arc::new(Fake), "fakeservice@main", "main", overrides).await
    }

    /// The spec `fakeservice@main` renders to with `idle_minutes` set to `minutes`.
    ///
    /// Writes the column rather than taking a second fixture parameter: the join under test is
    /// between a *row* and a recipe, and the three states of that column are the whole question.
    async fn idle_of(
        generator: &Generator,
        minutes: Option<i64>,
    ) -> Option<mixengine_proto::IdlePolicy> {
        sqlx::query("UPDATE services SET idle_minutes = ? WHERE id = 'fakeservice@main'")
            .bind(minutes)
            .execute(generator.store.pool())
            .await
            .expect("the column is written");

        generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a generated service")
            .spec
            .idle()
            .cloned()
    }

    /// The row decides how long, the recipe decides how, and `0` outranks both.
    ///
    /// Three assertions because the column has three states, and the third is the one that exists
    /// for **T70**'s sake rather than for anything reachable today: a default arriving in a later
    /// release must reach the home that never touched this setting and must not reach the one whose
    /// owner switched it off.
    #[tokio::test]
    async fn an_idle_policy_is_joined_from_the_row_and_the_recipe() {
        let (_home, generator) = home("{}").await;

        assert_eq!(
            idle_of(&generator, None).await,
            None,
            "a row that has not asked idles nothing, because no recipe here offers a default"
        );

        let policy = idle_of(&generator, Some(30))
            .await
            .expect("the row asked for a policy");

        assert_eq!(policy.after, mixengine_proto::Millis::from_secs(30 * 60));
        assert_eq!(
            policy.probe,
            mixengine_proto::IdleProbe::Connections { port: 4321 },
            "the probe is the recipe's, over the port the row allocated"
        );

        assert_eq!(
            idle_of(&generator, Some(0)).await,
            None,
            "zero minutes is never, not immediately"
        );
    }

    #[tokio::test]
    async fn a_row_becomes_a_spec_and_a_file_on_disk() {
        let (_home, generator) = home("{}").await;

        let generated = generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a generated service");

        assert_eq!(generated.spec.id().as_str(), "fakeservice@main");
        assert!(generated.changed(), "nothing was written on a first render");

        // **Roadmap task T71a**: the recipe's answer reaches the spec. This fixture says `true`
        // where the trait's default is `false`, so an assertion here fails if the generator stops
        // asking rather than passing for the wrong reason.
        assert!(
            generated.spec.restart_over_memory(),
            "the recipe's permission is carried onto the spec it renders"
        );

        let rendered = std::fs::read_to_string(
            generator
                .paths
                .etc()
                .join("fakeservice@main")
                .join("fakeservice.conf"),
        )
        .expect("the rendered file");

        assert!(rendered.contains("say = hello"), "{rendered}");
        assert!(rendered.contains("port = 4321"), "{rendered}");
    }

    /// **A number read without writing anything** — roadmap task **T53**.
    ///
    /// `mix cert status` connects to the front end's TLS port, and the two other ways to learn it
    /// are both refused. [`Generator::generate`] goes through [`Generator::declared`], which
    /// *installs*: a read-only status command would rewrite this home's whole configuration in
    /// order to read one number, and can reload a running server as a side effect of being asked a
    /// question. Parsing the rendered file back is what `.claude/CLAUDE.md` forbids outright.
    #[tokio::test]
    async fn a_services_settings_can_be_read_without_writing_anything() {
        let (_home, generator) = home(r##"{"greeting": "guten tag"}"##).await;
        let id = ServiceId::parse("fakeservice@main").expect("an id");

        let settings = generator.settings(&id).await.expect("its settings");

        assert_eq!(settings.text(GREETING), "guten tag");

        // The guarantee, and the whole reason this is a method rather than a field on `Generated`.
        assert!(
            !generator.paths.etc().join("fakeservice@main").exists(),
            "reading a setting installed a configuration"
        );
    }

    /// And a service nothing declares is `NotFound` rather than a default nobody set.
    #[tokio::test]
    async fn the_settings_of_a_service_that_does_not_exist_are_not_found() {
        let (_home, generator) = home("{}").await;
        let id = ServiceId::parse("fakeservice@other").expect("an id");

        assert!(matches!(
            generator.settings(&id).await,
            Err(Error::NotFound {
                kind: "service",
                ..
            })
        ));
    }

    #[tokio::test]
    async fn an_override_reaches_the_rendered_file() {
        let (_home, generator) = home(r##"{"greeting": "guten tag", "extra": "# mine\n"}"##).await;

        generator.declared().await.expect("the declared set");

        let rendered = std::fs::read_to_string(
            generator
                .paths
                .etc()
                .join("fakeservice@main")
                .join("fakeservice.conf"),
        )
        .expect("the rendered file");

        assert!(rendered.contains("say = guten tag"), "{rendered}");
        assert!(rendered.contains("# mine"), "{rendered}");
    }

    /// The second walk over an unchanged home is what the registry does on every `service.*` call.
    #[tokio::test]
    async fn generating_twice_changes_nothing_the_second_time() {
        let (_home, generator) = home("{}").await;

        generator.declared().await.expect("a first walk");
        let again = generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a second walk");

        assert!(!again.changed(), "an unchanged home rewrote its config");
    }

    /// The failure a home meets after an upgrade that dropped a recipe, and the one every home
    /// meets today for a package this build does not know.
    #[tokio::test]
    async fn a_package_with_no_recipe_names_itself_and_what_is_known() {
        let (_home, generator) = home("{}").await;

        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES ('meilisearch', '1.0.0', '/packages/x', '2026-08-15T00:00:00Z',
                     'https://example', 'ab');
             INSERT INTO services (id, package_id, instance_name, state)
             VALUES ('meilisearch@main', (SELECT id FROM packages WHERE name = 'meilisearch'),
                     'main', 'stopped')",
        )
        .execute(generator.store.pool())
        .await
        .expect("a service belonging to something this build cannot run");

        let error = generator
            .declared()
            .await
            .expect_err("a package with no recipe");

        let message = error.to_string();
        assert!(message.contains("meilisearch"), "{message}");
    }

    /// A singleton's data directory is `data/<package>`, and not `data/<package>/<package>`.
    ///
    /// The fallback was written for the case that has an instance name to spend. A recipe that
    /// exists once has no such half, and repeating the package name reads as a mistake to whoever
    /// meets it in a directory listing.
    #[tokio::test]
    async fn a_single_instance_recipe_keeps_its_data_directly_under_the_package() {
        let (_home, generator) = home_of(Arc::new(Solo), "solo", "solo", "{}").await;

        let generated = generator
            .generate(&ServiceId::parse("solo").expect("an id"))
            .await
            .expect("a generated service");

        assert_eq!(generated.spec.cwd(), generator.paths.data().join("solo"));
    }

    /// A named-instance recipe keeps the shape it always had: siblings under one package.
    #[tokio::test]
    async fn a_named_instance_recipe_keeps_its_data_under_the_instance() {
        let (_home, generator) = home("{}").await;

        let generated = generator
            .generate(&ServiceId::parse("fakeservice@main").expect("an id"))
            .await
            .expect("a generated service");

        assert_eq!(
            generated.spec.cwd(),
            generator.paths.data().join("fakeservice").join("main")
        );
    }

    /// A row that is fine except for its overrides fails as *that service*, not as a broken home.
    #[tokio::test]
    async fn a_misspelled_override_names_the_service_it_is_on() {
        let (_home, generator) = home(r#"{"greting": "hello"}"#).await;

        let error = generator
            .declared()
            .await
            .expect_err("a misspelled setting");
        let message = error.to_string();

        assert!(message.contains("fakeservice@main"), "{message}");
        assert!(message.contains("greting"), "{message}");
    }
    /// A service whose binary comes from an installed runtime renders exactly like one whose binary
    /// comes from a package.
    ///
    /// What is being asserted is the join and nothing else: the recipe is the same [`Fake`], the
    /// context it receives carries the runtime's version and install path, and **the name the recipe
    /// was found under is the id's own** — a pool is `php-fpm@8.3.33` and the row beneath it says
    /// `php`, which is the one place those two differ.
    #[tokio::test]
    async fn a_runtime_backed_row_renders_from_the_runtime_it_names() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let paths = Paths::new(directory.path().to_path_buf(), &PathOverrides::default());
        let store = Store::open(paths.database_file())
            .await
            .expect("a database");

        sqlx::query(
            r#"INSERT INTO runtime_installs
                   (kind, version, channel, install_path, installed_at, size_bytes, source_url,
                    sha256, provides_json)
               VALUES ('php', '8.3.33', 'stable', '/runtimes/php/8.3.33', '2026-08-19T00:00:00Z',
                       1, 'https://example.invalid/php', 'abc', '{"php-fpm":"sbin/php-fpm"}')"#,
        )
        .execute(store.pool())
        .await
        .expect("a runtime install");

        sqlx::query(
            "INSERT INTO services (id, runtime_install_id, instance_name, state, port)
             VALUES ('fakeservice@8.3.33', (SELECT id FROM runtime_installs LIMIT 1),
                     '8.3.33', 'stopped', 9000)",
        )
        .execute(store.pool())
        .await
        .expect("a service over it");

        let generator = Generator::new(
            paths.clone(),
            store,
            Catalogue::default().with(Arc::new(Fake)),
            Vec::new(),
        );

        let generated = generator.declared().await.expect("one rendered service");

        assert_eq!(
            generated.len(),
            1,
            "a row with no packages parent was dropped by the join"
        );
        assert_eq!(generated[0].spec.id().as_str(), "fakeservice@8.3.33");

        let rendered = std::fs::read_to_string(
            paths
                .etc()
                .join("fakeservice@8.3.33")
                .join("fakeservice.conf"),
        )
        .expect("the rendered file");

        assert!(rendered.contains("port = 9000"), "{rendered}");
    }
    /// The question `mix doctor`'s tenth check asks: is what is on disk what these rows render to?
    ///
    /// **The control is the first half.** A home that has never been rendered must drift, or an
    /// empty answer after the render is evidence of a blind comparison rather than of a clean home.
    #[tokio::test]
    async fn drift_is_something_before_a_render_and_nothing_after_one() {
        let (_home, generator) = home("{}").await;

        let before = generator.drift().await.expect("a drift");
        assert!(
            before.iter().any(|one| !one.drift.is_empty()),
            "a home that was never rendered reported no drift: {before:?}"
        );

        generator.declared().await.expect("a rendering");

        let after = generator.drift().await.expect("a second drift");
        assert!(
            after.iter().all(|one| one.drift.is_empty()),
            "a home that was just rendered still drifts: {after:?}"
        );
    }

    /// Asking must not install, or the check would repair what it was sent to report.
    #[tokio::test]
    async fn asking_for_drift_installs_nothing() {
        let (_home, generator) = home("{}").await;

        generator.drift().await.expect("a drift");

        let rendering = generator.declared().await.expect("a rendering");

        assert!(
            rendering[0].changed(),
            "the drift call had already written the configuration"
        );
    }

    /// A generated file somebody edited by hand is drift, and names the service it belongs to.
    #[tokio::test]
    async fn a_generated_file_edited_by_hand_drifts_under_its_own_service() {
        let (home_dir, generator) = home("{}").await;

        let rendered = generator.declared().await.expect("a rendering");
        let (file, _) = rendered[0]
            .files
            .first()
            .expect("the recipe renders at least one file");

        let quiet = generator.drift().await.expect("a drift");
        let service = quiet[0].service.clone();
        assert!(quiet[0].drift.is_empty(), "{quiet:?}");

        std::fs::write(
            home_dir
                .path()
                .join("etc")
                .join(service.as_str())
                .join(file),
            "tampered\n",
        )
        .expect("the generated file is writable");

        let after = generator.drift().await.expect("a second drift");

        assert_eq!(after[0].service, service);
        assert_eq!(after[0].drift.changed, vec![file.clone()]);
    }
}
