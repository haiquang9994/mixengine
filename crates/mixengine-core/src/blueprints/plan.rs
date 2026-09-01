//! What applying a blueprint would do, decided before anything happens.
//!
//! Roadmap task **T77**; **T78** is what carries the result out.
//!
//! # One place decides, and the order is part of the answer
//!
//! The feature's acceptance criterion is that `--dry-run` matches exactly what the real run
//! performs. That is only enforceable while one function decides what the actions are, so this is
//! it: T78's executor consumes the list and may **fail**, but may not add a step, drop one or
//! reorder them. The order here is dependency order — project, runtimes, services, databases, site,
//! domains, certificate, extensions, scaffold — and is asserted by a test rather than left to the
//! shape of the code.
//!
//! What is deliberately *not* in a plan is anything only the execution can know: the port a new
//! instance lands on, a generated password, a rowid. A plan that named them would be a plan the
//! executor has to contradict.
//!
//! # It reads this home's tables, and nothing else
//!
//! **No index, no network** (D9). The mismatch prompt in the feature doc shows a download size, and
//! a size means asking the index, and the index has a network behind a six-hour cache — while this
//! is the command a person runs *because* they do not want anything to happen yet. So a version
//! nothing installed satisfies is `create`, without a size, and whether the index still publishes it
//! is discovered by the real run.
//!
//! # Everything that cannot be done is decided here
//!
//! **D10.** The point of a plan is that an apply does not get five actions into a project directory
//! before discovering that the sixth was impossible — so a name that is too long, a directory that
//! is already another project's, and a domain another site owns are all `blocked` at this point,
//! each naming what stands in the way.

use std::collections::BTreeMap;
use std::path::Path;

use mixengine_proto::{
    AnswerSubject, BlueprintPlan, Disposition, MismatchAnswer, PackageVersion, PlanAction,
    PlanStep, RuntimeKind, ServiceId, SiteKind, VersionAnswer, VersionConstraint,
};

use crate::blueprints::manifest::{BlueprintManifest, PER_PROJECT};
use crate::{Result, Store, projects, runtimes, services, sites};

/// The token a manifest writes where the project's own name went.
const TOKEN: &str = "{project}";

/// The longest name MySQL and MariaDB accept for an account.
///
/// A limit belonging to somebody else, enforced here because that is the whole point of D10: a
/// 40-character project name produces a `CREATE USER` the server refuses, and finding that out
/// halfway through an apply is finding it out too late.
const DATABASE_USER_LIMIT: usize = 32;

/// What applying `manifest` under `project` would do.
///
/// # Errors
///
/// [`crate::Error::Database`] when a table cannot be read. Everything a person did wrong is a
/// [`Disposition::Blocked`] step rather than an error: a plan that refused to be printed would be a
/// plan that could not tell you *why*.
pub async fn plan(
    store: &Store,
    blueprint: &str,
    manifest: &BlueprintManifest,
    project: &str,
    root: &Path,
    answers: &[VersionAnswer],
) -> Result<BlueprintPlan> {
    let mut steps = Vec::new();

    // **Decided before the project step, because the pins it registers are what these answers
    // settle** (D7). The order of the steps themselves is unchanged: the runtimes are pushed
    // straight after the register, which is where they have always been.
    let mut runtimes = Vec::new();
    let mut pins = BTreeMap::new();

    for (kind, wanted) in &manifest.runtimes {
        let (step, pin) = runtime(store, *kind, wanted, answered_runtime(answers, *kind)).await?;

        pins.insert(*kind, pin);
        runtimes.push(step);
    }

    let (registered, mine) = register(store, project, root, pins).await?;
    steps.push(registered);
    steps.extend(runtimes);

    for service in &manifest.services {
        let instance =
            instance_of(store, &service.name, service.instance.as_deref(), project).await;
        let dedicated = service.instance.as_deref() == Some(PER_PROJECT);

        steps.push(package(store, &service.name, service.version.as_ref()).await?);
        steps.push(
            ensure(
                store,
                &service.name,
                &instance,
                service.version.as_ref(),
                dedicated,
                answers,
            )
            .await?,
        );

        if let Some(database) = &service.database {
            let user = service.user.clone().unwrap_or_else(|| database.clone());
            steps.push(database_step(
                &service.name,
                &instance,
                &expand(database, project),
                &expand(&user, project),
            ));
        }
    }

    if let Some(site) = &manifest.site {
        steps.push(PlanStep {
            action: PlanAction::CreateSite {
                kind: match &site.kind {
                    // Which pool a new site uses is decided on the machine that makes it.
                    SiteKind::PhpFpm { .. } => SiteKind::PhpFpm { pool: None },
                    other => other.clone(),
                },
                doc_root: site.doc_root.clone(),
                https: site.https,
            },
            disposition: Disposition::Create,
            elevates: false,
        });

        let mut names = Vec::new();
        names.push(expand(&site.domain_pattern, project));
        names.extend(site.aliases.iter().map(|alias| expand(alias, project)));

        for (position, domain) in names.iter().enumerate() {
            steps.push(domain_step(store, domain, position == 0, mine).await?);
        }

        if site.https {
            steps.push(PlanStep {
                action: PlanAction::IssueCertificate {
                    domains: names.clone(),
                },
                disposition: Disposition::Create,
                // On a machine that has never issued one, this installs the authority.
                elevates: true,
            });
        }
    }

    if let Some(php) = &manifest.php {
        let installed = newest(store, RuntimeKind::Php).await?;

        for name in &php.extensions {
            steps.push(extension(store, installed.as_ref(), name).await?);
        }
    }

    if let Some(scaffold) = &manifest.scaffold {
        steps.push(PlanStep {
            action: PlanAction::RunScaffold {
                command: scaffold.command.clone(),
            },
            // Arbitrary code from whoever wrote the blueprint. T78a is what gates it; here it is
            // shown, exactly as it would run.
            disposition: Disposition::Confirm {
                what: scaffold.command.clone(),
            },
            elevates: false,
        });
    }

    Ok(BlueprintPlan {
        blueprint: blueprint.to_owned(),
        project: project.to_owned(),
        root: root.display().to_string(),
        steps,
    })
}

/// The project itself: its name, the versions it will ask for, and whether this directory is free.
///
/// Answers with the rowid of the project this apply is *about* where there already is one, which is
/// what [`domain_step`] needs to tell a name this apply already claimed from a name somebody else
/// holds.
async fn register(
    store: &Store,
    project: &str,
    root: &Path,
    pins: BTreeMap<RuntimeKind, VersionConstraint>,
) -> Result<(PlanStep, Option<i64>)> {
    let action = PlanAction::RegisterProject {
        name: project.to_owned(),
        root: root.display().to_string(),
        pins,
    };

    if let Err(error) = projects::validated_name(project) {
        return Ok((blocked(action, error.to_string()), None));
    }

    let registered = projects::records(store).await?;
    let here = mixengine_platform::paths::in_full(root);

    // **Resumption is a re-plan** (the T78 design, D2). A project of this name *at this root* is
    // this apply's own first step, already taken, and calling that a collision would make a failed
    // apply impossible to run again. Both halves have to match: anything narrower is two projects
    // colliding, which is what the blocks below are for.
    if let Some(mine) = registered.iter().find(|record| {
        record.name == project && mixengine_platform::paths::in_full(&record.root) == here
    }) {
        return Ok((satisfied(action), Some(mine.id)));
    }

    if registered.iter().any(|record| record.name == project) {
        return Ok((
            blocked(
                action,
                format!("a project called {project} is already registered elsewhere"),
            ),
            None,
        ));
    }

    if let Some(holder) = registered
        .iter()
        .find(|record| mixengine_platform::paths::in_full(&record.root) == here)
    {
        return Ok((
            blocked(
                action,
                format!("{} is already the project {}", root.display(), holder.name),
            ),
            None,
        ));
    }

    Ok((
        PlanStep {
            action,
            disposition: Disposition::Create,
            elevates: false,
        },
        None,
    ))
}

/// The answer for one language, where somebody gave one.
fn answered_runtime(answers: &[VersionAnswer], kind: RuntimeKind) -> Option<MismatchAnswer> {
    answers.iter().find_map(|given| match &given.subject {
        AnswerSubject::Runtime { kind: asked } if *asked == kind => Some(given.answer),
        _ => None,
    })
}

/// The answer for one instance, where somebody gave one.
fn answered_service(answers: &[VersionAnswer], id: &ServiceId) -> Option<MismatchAnswer> {
    answers.iter().find_map(|given| match &given.subject {
        AnswerSubject::Service { id: asked } if asked == id => Some(given.answer),
        _ => None,
    })
}

/// One `[runtimes]` entry against what is installed, and the pin the project gets from it.
///
/// **The pin is half of the answer** (D7). Without it "install 8.2.23" and "use the installed
/// 8.2.29" would leave identical machines behind and the question would be theatre.
async fn runtime(
    store: &Store,
    kind: RuntimeKind,
    wanted: &VersionConstraint,
    answer: Option<MismatchAnswer>,
) -> Result<(PlanStep, VersionConstraint)> {
    let action = PlanAction::InstallRuntime {
        kind,
        wanted: wanted.clone(),
    };

    let installed = runtimes::records(store, Some(kind)).await?;

    if installed
        .iter()
        .any(|record| wanted.matches(&record.version))
    {
        return Ok((satisfied(action), wanted.clone()));
    }

    // Newest first, because that is the one an "use what is installed" answer would take.
    let newest = installed
        .into_iter()
        .max_by(|left, right| left.version.cmp_precedence(&right.version));

    Ok(match (newest, answer) {
        // Nothing to choose between: there is no question here, whatever anybody answered.
        (None, _) => (
            PlanStep {
                action,
                disposition: Disposition::Create,
                elevates: false,
            },
            wanted.clone(),
        ),

        (Some(record), None) => (
            PlanStep {
                action,
                disposition: Disposition::Choice {
                    installed: record.version,
                    wanted: wanted.clone(),
                },
                elevates: false,
            },
            wanted.clone(),
        ),

        (Some(_), Some(MismatchAnswer::Install)) => (
            PlanStep {
                action,
                disposition: Disposition::Create,
                elevates: false,
            },
            wanted.clone(),
        ),

        // A version this store holds is one it could name a directory after, so it is a constraint
        // too; the fallback is unreachable rather than lenient.
        (Some(record), Some(MismatchAnswer::UseInstalled)) => (
            satisfied(action),
            VersionConstraint::parse(record.version.as_str()).unwrap_or_else(|_| wanted.clone()),
        ),
    })
}

/// Which instance name a `[[services]]` entry means on *this* machine.
///
/// `per-project` becomes the new project's own name. An absent instance follows the lookup
/// [`crate::manifest`] documents — the bare package name first, which is what a single-instance
/// package such as `caddy` is actually called, and `main` after it.
///
/// The pair may still be unspellable as a [`ServiceId`] — a project called `My Blog` cannot name an
/// instance — and that is [`ensure`]'s to report, not this function's to paper over.
async fn instance_of(store: &Store, name: &str, instance: Option<&str>, project: &str) -> String {
    match instance {
        Some(PER_PROJECT) => project.to_owned(),
        Some(named) => named.to_owned(),
        None => match ServiceId::parse(name) {
            Ok(bare) if services::record(store, &bare).await.is_ok() => name.to_owned(),
            _ => "main".to_owned(),
        },
    }
}

/// The id that pair would have, or [`None`] when it cannot be spelled as one.
fn identity(package: &str, instance: &str) -> Option<ServiceId> {
    ServiceId::parse(package)
        .ok()
        .filter(|bare| bare.as_str() == instance)
        .or_else(|| ServiceId::parse(format!("{package}@{instance}")).ok())
}

/// Whether the package an instance would run is on this disk at all.
///
/// **T77 planned this for languages and not for services** (D8), and `service.create` refuses with
/// `precondition_failed` when the version it is asked for is not installed — a plan discovering the
/// impossible five actions into a project directory, which is the whole thing a plan exists to
/// prevent, and the ordinary case for a blueprint that came from somebody else's machine.
///
/// **It never asks a question.** A version mismatch is a question about an *instance* that already
/// exists, and [`ensure`] is where it is asked; where there is no instance yet there is nothing to
/// reuse and nothing to choose between.
async fn package(
    store: &Store,
    name: &str,
    wanted: Option<&VersionConstraint>,
) -> Result<PlanStep> {
    let action = PlanAction::InstallPackage {
        package: name.to_owned(),
        wanted: wanted.cloned(),
    };

    let installed = crate::packages::records(store, Some(name)).await?;

    let have = match wanted {
        Some(wanted) => installed
            .iter()
            .any(|record| wanted.matches(&record.version)),
        // Nothing pinned: any version of it is the version this blueprint asked for.
        None => !installed.is_empty(),
    };

    Ok(match have {
        true => satisfied(action),
        false => PlanStep {
            action,
            disposition: Disposition::Create,
            elevates: false,
        },
    })
}

/// Whether that instance is already here, and at the right version.
async fn ensure(
    store: &Store,
    package: &str,
    instance: &str,
    wanted: Option<&VersionConstraint>,
    dedicated: bool,
    answers: &[VersionAnswer],
) -> Result<PlanStep> {
    let action = PlanAction::EnsureService {
        package: package.to_owned(),
        instance: instance.to_owned(),
        version: wanted.cloned(),
        dedicated,
    };

    // **D10.** A pair no id can be spelled from is decided here, with the reason, rather than at
    // the moment T78 tries to write the row.
    let Some(id) = identity(package, instance) else {
        return Ok(blocked(
            action,
            format!("{package}@{instance} cannot be a service id"),
        ));
    };

    if services::record(store, &id).await.is_err() {
        return Ok(PlanStep {
            action,
            disposition: Disposition::Create,
            elevates: false,
        });
    }

    let installed = services::version(store, &id).await?;

    Ok(match (wanted, installed) {
        (Some(wanted), Some(installed)) if !wanted.matches(&installed) => {
            match answered_service(answers, &id) {
                // Reusing what is here is the one thing this build can do about it.
                Some(MismatchAnswer::UseInstalled) => satisfied(action),

                // **A blocked step and not an error.** Repointing an existing instance at another
                // version is a database upgrade under somebody's data directory, and this build has
                // no method for it: `service.create` and `service.delete` are the two ends of a
                // row's life with nothing between them. Said here, which is where every other
                // impossibility is said (D10).
                Some(MismatchAnswer::Install) => blocked(
                    action,
                    format!(
                        "{id} is already running {installed}, and this build cannot move an \
                         existing instance to another version — answer `use_installed` to reuse \
                         it, or give the blueprint `instance = \"per-project\"` for one of its own"
                    ),
                ),

                None => PlanStep {
                    action,
                    disposition: Disposition::Choice {
                        installed,
                        wanted: wanted.clone(),
                    },
                    elevates: false,
                },
            }
        }
        _ => satisfied(action),
    })
}

/// The database and the account, or the reason neither can be made under this name.
fn database_step(package: &str, instance: &str, database: &str, user: &str) -> PlanStep {
    let action = PlanAction::CreateDatabase {
        package: package.to_owned(),
        instance: instance.to_owned(),
        database: database.to_owned(),
        user: user.to_owned(),
    };

    match user.len() > DATABASE_USER_LIMIT {
        true => blocked(
            action,
            format!(
                "{user} is longer than the {DATABASE_USER_LIMIT} characters a database account may have"
            ),
        ),
        false => PlanStep {
            action,
            disposition: Disposition::Create,
            elevates: false,
        },
    }
}

/// One name, and who already answers to it.
async fn domain_step(
    store: &Store,
    domain: &str,
    primary: bool,
    project: Option<i64>,
) -> Result<PlanStep> {
    let action = PlanAction::AddDomain {
        domain: domain.to_owned(),
        primary,
    };

    Ok(match sites::by_domain(store, domain).await? {
        // Already ours, which is what a resumed apply looks like from here (D2). Narrower than "the
        // name is taken" by exactly one condition, and that condition is the whole difference
        // between running an apply twice and being told to go away.
        Some(owner) if Some(owner.project_id) == project => satisfied(action),

        Some(owner) => blocked(
            action,
            format!(
                "{domain} is already answered by {}",
                owner
                    .domains
                    .first()
                    .map_or("another site", |primary| primary.as_str())
            ),
        ),
        // Writing the hosts file is what needs the prompt, and saying so before anything starts is
        // the point of D11.
        None => PlanStep {
            action,
            disposition: Disposition::Create,
            elevates: true,
        },
    })
}

/// One extension against the PHP that would run it.
async fn extension(
    store: &Store,
    installed: Option<&PackageVersion>,
    name: &str,
) -> Result<PlanStep> {
    let Some(version) = installed else {
        // Nothing to enable it on yet; the runtime step above already says the PHP is coming.
        return Ok(PlanStep {
            action: PlanAction::SetPhpExtension {
                runtime: PackageVersion::parse("0.0.0").expect("a placeholder version"),
                name: name.to_owned(),
            },
            disposition: Disposition::Create,
            elevates: false,
        });
    };

    let action = PlanAction::SetPhpExtension {
        runtime: version.clone(),
        name: name.to_owned(),
    };

    let state = crate::runtimes::extensions::state(store, RuntimeKind::Php, version).await?;

    Ok(match state.loaded().iter().any(|loaded| loaded == name) {
        true => satisfied(action),
        false => PlanStep {
            action,
            disposition: Disposition::Create,
            elevates: false,
        },
    })
}

/// The newest installed version of a language, where there is one.
async fn newest(store: &Store, kind: RuntimeKind) -> Result<Option<PackageVersion>> {
    Ok(runtimes::records(store, Some(kind))
        .await?
        .into_iter()
        .max_by(|left, right| left.version.cmp_precedence(&right.version))
        .map(|record| record.version))
}

/// `{project}` becomes the new project's name. **Once, here**, so no later branch can expand it
/// differently.
fn expand(value: &str, project: &str) -> String {
    value.replace(TOKEN, project)
}

/// A step that needs nothing done.
fn satisfied(action: PlanAction) -> PlanStep {
    PlanStep {
        action,
        disposition: Disposition::Satisfied,
        elevates: false,
    }
}

/// A step that cannot be done, and why.
fn blocked(action: PlanAction, reason: String) -> PlanStep {
    PlanStep {
        action,
        disposition: Disposition::Blocked { reason },
        elevates: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use crate::blueprints::manifest::{BlueprintService, BlueprintSite, Header, Php, Provenance};

    async fn home() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a database");

        (temp, store)
    }

    fn a_manifest() -> BlueprintManifest {
        BlueprintManifest {
            schema: crate::blueprints::manifest::SCHEMA,
            blueprint: Header {
                name: "blog-stack".to_owned(),
                description: String::new(),
                created_at: "2026-09-01T09:00:00Z".to_owned(),
                created_on: Provenance {
                    os: "linux".to_owned(),
                    version: "0.1.0".to_owned(),
                },
            },
            runtimes: [(
                RuntimeKind::Php,
                VersionConstraint::parse("8.2.23").expect("a constraint"),
            )]
            .into_iter()
            .collect(),
            site: Some(BlueprintSite {
                kind: SiteKind::PhpFpm { pool: None },
                doc_root: "public".to_owned(),
                https: true,
                domain_pattern: "{project}.test".to_owned(),
                aliases: Vec::new(),
            }),
            services: vec![BlueprintService {
                name: "mariadb".to_owned(),
                version: Some(VersionConstraint::parse("11.4.3").expect("a constraint")),
                instance: Some("main".to_owned()),
                database: Some("{project}".to_owned()),
                user: Some("{project}".to_owned()),
            }],
            php: Some(Php {
                extensions: vec!["xdebug".to_owned()],
            }),
            scaffold: None,
        }
    }

    async fn an_installed_php(store: &Store, version: &str, choices: &str) {
        sqlx::query(
            r#"INSERT INTO runtime_installs
                   (id, kind, version, channel, install_path, installed_at, size_bytes, source_url,
                    sha256, extension_choices_json, extensions_json)
               VALUES (1, 'php', ?1, 'stable', '/runtimes/php', '2026-09-01T00:00:00Z', 1,
                       'https://example.invalid/php', 'ab', ?2,
                       '{"shared":["xdebug"],"enabled":[],"compiled_in":[]}')"#,
        )
        .bind(version)
        .bind(choices)
        .execute(store.pool())
        .await
        .expect("a runtime install");
    }

    async fn an_installed_mariadb(store: &Store, version: &str) {
        sqlx::query(
            "INSERT INTO packages (id, name, version, install_path, installed_at, source_url, sha256)
             VALUES (1, 'mariadb', ?1, '/packages/mariadb', '2026-09-01T00:00:00Z',
                     'https://example.invalid/m', 'ab')",
        )
        .bind(version)
        .execute(store.pool())
        .await
        .expect("a package");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state, port)
             VALUES ('mariadb@main', 1, 'main', 'stopped', 3306)",
        )
        .execute(store.pool())
        .await
        .expect("a service");
    }

    fn step_of(planned: &BlueprintPlan, wanted: impl Fn(&PlanAction) -> bool) -> &PlanStep {
        planned
            .steps
            .iter()
            .find(|step| wanted(&step.action))
            .expect("the step")
    }

    /// Everything the blueprint needs is already here, so only the new project's own things are
    /// created — and `{project}` is expanded exactly once, into the domain.
    #[tokio::test]
    async fn a_home_that_already_has_everything_needs_nothing_installed() {
        let (temp, store) = home().await;
        an_installed_php(&store, "8.2.23", r#"{"xdebug":true}"#).await;
        an_installed_mariadb(&store, "11.4.3").await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::InstallRuntime { .. }
            ))
            .disposition,
            Disposition::Satisfied
        );
        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::EnsureService { .. }
            ))
            .disposition,
            Disposition::Satisfied
        );
        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::SetPhpExtension { .. }
            ))
            .disposition,
            Disposition::Satisfied
        );

        let PlanAction::AddDomain { domain, .. } = &step_of(&planned, |action| {
            matches!(action, PlanAction::AddDomain { .. })
        })
        .action
        else {
            panic!("a domain step");
        };
        assert_eq!(domain, "shop.test");

        let PlanAction::CreateDatabase { database, user, .. } = &step_of(&planned, |action| {
            matches!(action, PlanAction::CreateDatabase { .. })
        })
        .action
        else {
            panic!("a database step");
        };
        assert_eq!((database.as_str(), user.as_str()), ("shop", "shop"));
    }

    /// A different patch release is a question for a person, not a decision for the daemon.
    #[tokio::test]
    async fn another_version_of_an_installed_runtime_is_a_choice_rather_than_an_install() {
        let (temp, store) = home().await;
        an_installed_php(&store, "8.2.29", "{}").await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert!(
            matches!(
                &step_of(&planned, |action| matches!(
                    action,
                    PlanAction::InstallRuntime { .. }
                ))
                .disposition,
                Disposition::Choice { installed, .. } if installed.as_str() == "8.2.29"
            ),
            "{planned:?}"
        );
    }

    /// Nothing of that language installed is an install, without a size: the plan never asks the
    /// index (D9).
    #[tokio::test]
    async fn a_runtime_this_home_does_not_have_is_an_install() {
        let (temp, store) = home().await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::InstallRuntime { .. }
            ))
            .disposition,
            Disposition::Create
        );
    }

    /// **D10.** The owner is named, because "taken" without a name sends somebody hunting.
    #[tokio::test]
    async fn a_domain_another_site_owns_is_blocked_and_says_who_has_it() {
        let (temp, store) = home().await;

        let project = crate::projects::create(
            &store,
            &crate::projects::Registration {
                name: "blog".to_owned(),
                root: temp.path().join("blog"),
                pins: BTreeMap::new(),
            },
            mixengine_proto::Timestamp::from_system_time(std::time::SystemTime::UNIX_EPOCH),
        )
        .await
        .expect("a project");

        crate::sites::create(
            &store,
            &crate::sites::NewSite {
                project_id: project.id,
                doc_root: String::new(),
                kind: SiteKind::Static,
                https_enabled: true,
                domains: vec!["shop.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site holding the name");

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        let Disposition::Blocked { reason } = &step_of(&planned, |action| {
            matches!(action, PlanAction::AddDomain { .. })
        })
        .disposition
        else {
            panic!("the domain step should be blocked: {planned:?}");
        };

        assert!(reason.contains("shop.test"), "{reason}");
    }

    /// **D10 again**: a name whose expansion cannot be a database account is refused at dry-run,
    /// not five actions into an apply.
    #[tokio::test]
    async fn a_project_name_too_long_for_a_database_account_is_blocked_here() {
        let (temp, store) = home().await;
        let long = "a".repeat(40);

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            &long,
            &temp.path().join("long"),
            &[],
        )
        .await
        .expect("a plan");

        assert!(matches!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::CreateDatabase { .. }
            ))
            .disposition,
            Disposition::Blocked { .. }
        ));
    }

    /// **D8.** The order is what T78 executes, so it is asserted rather than left to chance.
    #[tokio::test]
    async fn the_steps_are_in_dependency_order() {
        let (temp, store) = home().await;
        let mut manifest = a_manifest();
        manifest.scaffold = Some(crate::blueprints::manifest::Scaffold {
            command: "composer create-project laravel/laravel .".to_owned(),
        });

        let planned = plan(
            &store,
            "blog-stack",
            &manifest,
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        let rank = |action: &PlanAction| match action {
            PlanAction::RegisterProject { .. } => 0,
            PlanAction::InstallRuntime { .. } => 1,
            PlanAction::InstallPackage { .. } => 2,
            PlanAction::EnsureService { .. } => 3,
            PlanAction::CreateDatabase { .. } => 4,
            PlanAction::CreateSite { .. } => 5,
            PlanAction::AddDomain { .. } => 6,
            PlanAction::IssueCertificate { .. } => 7,
            PlanAction::SetPhpExtension { .. } => 8,
            PlanAction::RunScaffold { .. } => 9,
            _ => 10,
        };

        let ranks: Vec<_> = planned
            .steps
            .iter()
            .map(|step| rank(&step.action))
            .collect();

        assert!(
            ranks.windows(2).all(|pair| pair[0] <= pair[1]),
            "{ranks:?} is not dependency order"
        );
    }

    /// **D8.** A machine with no MariaDB at all is the ordinary case for the feature's headline
    /// scenario — a blueprint from somebody else's machine — and the plan says so before anything is
    /// written, rather than letting `service.create` refuse halfway through.
    #[tokio::test]
    async fn a_package_this_home_does_not_have_is_planned_as_an_install() {
        let (temp, store) = home().await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        let step = step_of(&planned, |action| {
            matches!(action, PlanAction::InstallPackage { .. })
        });
        assert_eq!(step.disposition, Disposition::Create);

        let PlanAction::InstallPackage { package, wanted } = &step.action else {
            panic!("an install step");
        };
        assert_eq!(package, "mariadb");
        assert_eq!(
            wanted.as_ref().map(VersionConstraint::as_str),
            Some("11.4.3")
        );
    }

    /// And a home that already has it needs nothing.
    #[tokio::test]
    async fn a_package_already_on_disk_needs_no_install() {
        let (temp, store) = home().await;
        an_installed_mariadb(&store, "11.4.3").await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::InstallPackage { .. }
            ))
            .disposition,
            Disposition::Satisfied
        );
    }

    /// A project registered at `root`, as the two resumption tests need one.
    async fn a_project(store: &Store, name: &str, root: &std::path::Path) -> i64 {
        std::fs::create_dir_all(root).expect("a directory");

        crate::projects::create(
            store,
            &crate::projects::Registration {
                name: name.to_owned(),
                root: root.to_path_buf(),
                pins: BTreeMap::new(),
            },
            mixengine_proto::Timestamp::from_system_time(std::time::SystemTime::UNIX_EPOCH),
        )
        .await
        .expect("a project")
        .id
    }

    /// **D2.** A second apply of the same blueprint is how a failed one is resumed, so the project
    /// this apply would make — already made, at this root — is not a collision. It is the first half
    /// of this apply having succeeded.
    #[tokio::test]
    async fn the_project_this_apply_already_made_is_satisfied_rather_than_blocked() {
        let (temp, store) = home().await;
        let root = temp.path().join("shop");
        a_project(&store, "shop", &root).await;

        let planned = plan(&store, "blog-stack", &a_manifest(), "shop", &root, &[])
            .await
            .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::RegisterProject { .. }
            ))
            .disposition,
            Disposition::Satisfied
        );
    }

    /// And the narrowness is the point: the same name somewhere else is still two projects
    /// colliding, which is what the block was written for.
    #[tokio::test]
    async fn the_same_name_at_another_root_is_still_blocked() {
        let (temp, store) = home().await;
        a_project(&store, "shop", &temp.path().join("elsewhere")).await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert!(matches!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::RegisterProject { .. }
            ))
            .disposition,
            Disposition::Blocked { .. }
        ));
    }

    /// A name this project's own site already answers to is done, not taken — which is what makes
    /// the domain half of a resumed apply pass. Any other site holding it is still `Blocked`, and
    /// the test above this one says so.
    #[tokio::test]
    async fn a_domain_this_projects_own_site_holds_is_satisfied() {
        let (temp, store) = home().await;
        let root = temp.path().join("shop");
        let project = a_project(&store, "shop", &root).await;

        sites::create(
            &store,
            &sites::NewSite {
                project_id: project,
                doc_root: "public".to_owned(),
                kind: SiteKind::PhpFpm { pool: None },
                https_enabled: true,
                domains: vec!["shop.test".to_owned()],
                services: Vec::new(),
            },
        )
        .await
        .expect("a site");

        let planned = plan(&store, "blog-stack", &a_manifest(), "shop", &root, &[])
            .await
            .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::AddDomain { .. }
            ))
            .disposition,
            Disposition::Satisfied
        );
    }

    /// **D7.** The project a blueprint makes is pinned to what the blueprint asked for; without
    /// this the site resolves to whatever PHP this machine defaults to, and a capture of the new
    /// project comes back with no `[runtimes]` at all.
    #[tokio::test]
    async fn the_project_is_registered_with_the_pins_the_blueprint_asks_for() {
        let (temp, store) = home().await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        let PlanAction::RegisterProject { pins, .. } = &step_of(&planned, |action| {
            matches!(action, PlanAction::RegisterProject { .. })
        })
        .action
        else {
            panic!("a register step");
        };

        assert_eq!(
            pins.get(&RuntimeKind::Php).map(VersionConstraint::as_str),
            Some("8.2.23")
        );
    }

    /// **D6 and D7 are one decision.** "Use the installed one" is not just a skipped download: it is
    /// what the project asks for from now on, or the two answers would leave identical machines
    /// behind and the question would be theatre.
    #[tokio::test]
    async fn answering_use_installed_pins_the_project_to_the_version_this_machine_has() {
        let (temp, store) = home().await;
        an_installed_php(&store, "8.2.29", "{}").await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[VersionAnswer {
                subject: AnswerSubject::Runtime {
                    kind: RuntimeKind::Php,
                },
                answer: MismatchAnswer::UseInstalled,
            }],
        )
        .await
        .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::InstallRuntime { .. }
            ))
            .disposition,
            Disposition::Satisfied,
            "the answer settles the question, so nothing is left to ask"
        );

        let PlanAction::RegisterProject { pins, .. } = &step_of(&planned, |action| {
            matches!(action, PlanAction::RegisterProject { .. })
        })
        .action
        else {
            panic!("a register step");
        };

        assert_eq!(
            pins.get(&RuntimeKind::Php).map(VersionConstraint::as_str),
            Some("8.2.29"),
            "the pin follows the answer"
        );
    }

    /// The other answer leaves the pin where the blueprint put it, and turns the question into work.
    #[tokio::test]
    async fn answering_install_keeps_the_blueprints_own_pin() {
        let (temp, store) = home().await;
        an_installed_php(&store, "8.2.29", "{}").await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[VersionAnswer {
                subject: AnswerSubject::Runtime {
                    kind: RuntimeKind::Php,
                },
                answer: MismatchAnswer::Install,
            }],
        )
        .await
        .expect("a plan");

        assert_eq!(
            step_of(&planned, |action| matches!(
                action,
                PlanAction::InstallRuntime { .. }
            ))
            .disposition,
            Disposition::Create
        );

        let PlanAction::RegisterProject { pins, .. } = &step_of(&planned, |action| {
            matches!(action, PlanAction::RegisterProject { .. })
        })
        .action
        else {
            panic!("a register step");
        };

        assert_eq!(
            pins.get(&RuntimeKind::Php).map(VersionConstraint::as_str),
            Some("8.2.23")
        );
    }

    /// **An existing instance cannot be moved to another version by this build**, and the plan is
    /// where that is said — a blocked step naming the way out, rather than a failure five actions
    /// into a project directory (D10).
    #[tokio::test]
    async fn asking_to_install_over_an_existing_instance_is_blocked_and_names_the_other_answer() {
        let (temp, store) = home().await;
        an_installed_mariadb(&store, "11.4.5").await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[VersionAnswer {
                subject: AnswerSubject::Service {
                    id: ServiceId::parse("mariadb@main").expect("an id"),
                },
                answer: MismatchAnswer::Install,
            }],
        )
        .await
        .expect("a plan");

        assert!(
            matches!(
                &step_of(&planned, |action| matches!(
                    action,
                    PlanAction::EnsureService { .. }
                ))
                .disposition,
                Disposition::Blocked { reason } if reason.contains("use_installed")
            ),
            "{planned:?}"
        );
    }

    /// A scaffold command is shown, exactly as it would run, and agreed to rather than done.
    #[tokio::test]
    async fn a_scaffold_command_is_something_to_agree_to() {
        let (temp, store) = home().await;
        let mut manifest = a_manifest();
        manifest.scaffold = Some(crate::blueprints::manifest::Scaffold {
            command: "composer create-project laravel/laravel .".to_owned(),
        });

        let planned = plan(
            &store,
            "blog-stack",
            &manifest,
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert!(matches!(
            &step_of(&planned, |action| matches!(
                action,
                PlanAction::RunScaffold { .. }
            ))
            .disposition,
            Disposition::Confirm { what } if what.contains("composer")
        ));
    }

    /// **D11.** Adding a domain writes the hosts file, and nothing else in a plan asks for a
    /// password.
    #[tokio::test]
    async fn only_the_steps_that_need_a_password_say_so() {
        let (temp, store) = home().await;

        let planned = plan(
            &store,
            "blog-stack",
            &a_manifest(),
            "shop",
            &temp.path().join("shop"),
            &[],
        )
        .await
        .expect("a plan");

        assert!(
            planned
                .steps
                .iter()
                .any(|step| step.elevates && matches!(step.action, PlanAction::AddDomain { .. }))
        );
        assert!(planned.steps.iter().all(|step| !step.elevates
            || matches!(
                step.action,
                PlanAction::AddDomain { .. } | PlanAction::IssueCertificate { .. }
            )));
    }

    /// **D10, and the case a project name makes reachable.** Project names allow spaces and upper
    /// case; service ids do not. A dedicated instance for `My Blog` cannot be spelled, and that is
    /// said here rather than discovered by T78 while writing the row.
    #[tokio::test]
    async fn a_project_name_no_service_id_can_hold_blocks_its_dedicated_instance() {
        let (temp, store) = home().await;
        let mut manifest = a_manifest();
        manifest.services[0].instance = Some(PER_PROJECT.to_owned());

        let planned = plan(
            &store,
            "blog-stack",
            &manifest,
            "My Blog",
            &temp.path().join("my blog"),
            &[],
        )
        .await
        .expect("a plan");

        assert!(
            matches!(
                &step_of(&planned, |action| matches!(
                    action,
                    PlanAction::EnsureService { .. }
                ))
                .disposition,
                Disposition::Blocked { reason } if reason.contains("mariadb@My Blog")
            ),
            "{planned:?}"
        );
    }

    /// A directory that is already a project is not a place to put a second one.
    #[tokio::test]
    async fn a_root_that_is_already_a_project_is_blocked() {
        let (temp, store) = home().await;
        let root = temp.path().join("blog");
        std::fs::create_dir_all(&root).expect("a directory");

        crate::projects::create(
            &store,
            &crate::projects::Registration {
                name: "blog".to_owned(),
                root: root.clone(),
                pins: BTreeMap::new(),
            },
            mixengine_proto::Timestamp::from_system_time(std::time::SystemTime::UNIX_EPOCH),
        )
        .await
        .expect("a project");

        let planned = plan(&store, "blog-stack", &a_manifest(), "shop", &root, &[])
            .await
            .expect("a plan");

        assert!(
            matches!(
                &step_of(&planned, |action| matches!(
                    action,
                    PlanAction::RegisterProject { .. }
                ))
                .disposition,
                Disposition::Blocked { reason } if reason.contains("blog")
            ),
            "{planned:?}"
        );
    }
}
