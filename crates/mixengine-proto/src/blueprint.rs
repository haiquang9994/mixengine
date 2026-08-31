//! Blueprints on the wire: what one is, and what applying one would do.
//!
//! Roadmap task **T77**. **The plan is a value, not a paragraph** — the T77 design, D8. `mix`
//! renders these steps and a graphical client renders the same ones differently; neither decides
//! what is in the list. And the plan is what T78 executes, which is the only way `--dry-run` can
//! promise to match the real run: one place decides the set and the order, and the executor may
//! fail but may not add a step, drop one or reorder them.

use crate::{PackageVersion, RuntimeKind, ServiceId, SiteKind};

/// One blueprint, as a listing shows it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintSummary {
    /// The handle: what a person types, what the row's primary key is, and the file's stem.
    pub slug: String,

    /// The display name.
    ///
    /// Equal to [`Self::slug`] for a captured blueprint. The two are separate because the gallery
    /// (T79) ships blueprints whose title is not a handle, and one column serving both would have
    /// to pick which of the two it lies about.
    pub name: String,

    /// What it is for, or empty.
    pub description: String,

    /// When it was captured, ISO-8601 UTC.
    pub created_at: String,

    /// Where it came from.
    pub source: BlueprintSource,

    /// The rendered file.
    ///
    /// Carried in the listing so that reading the TOML needs no `blueprint.get`: the row is the
    /// truth and this file is its rendering (D7), so pointing at it is the whole of what a
    /// "show me" method would have done.
    pub file: String,
}

/// Where a blueprint came from.
///
/// Three words rather than a boolean, because T78a's trust marking is what reads this: a
/// hand-imported blueprint carries arbitrary scaffold code and stays untrusted for good, and
/// "not built in" would not distinguish it from one captured on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlueprintSource {
    /// Shipped with MixEngine and signed.
    Builtin,

    /// Written by `blueprint.capture` on this machine.
    Captured,

    /// Brought in from a file somebody else wrote.
    Imported,
}

impl BlueprintSource {
    /// Every source, in the order a listing would group them.
    pub const ALL: [Self; 3] = [Self::Builtin, Self::Captured, Self::Imported];

    /// The word the `blueprints.source` column holds.
    ///
    /// One spelling for the column and the wire, on [`RuntimeKind::as_str`]'s rule: a second one
    /// would be a second vocabulary to keep in step.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::Captured => "captured",
            Self::Imported => "imported",
        }
    }

    /// Read one back, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|source| source.as_str() == value)
    }
}

/// What `blueprint.list` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintList {
    /// Every blueprint this home holds, in slug order.
    pub blueprints: Vec<BlueprintSummary>,
}

/// What applying a blueprint would do, decided before anything happens.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintPlan {
    /// Which blueprint, by slug.
    pub blueprint: String,

    /// The name the new project would have, which is also what `{project}` was expanded to.
    pub project: String,

    /// Where it would live.
    pub root: String,

    /// Every action, in the order it would be carried out.
    pub steps: Vec<PlanStep>,
}

/// One action and what this home makes of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanStep {
    /// What would be done.
    pub action: PlanAction,

    /// Whether it needs doing, and what stands in the way.
    pub disposition: Disposition,

    /// Whether carrying it out asks the OS for an elevation prompt — the T77 design, D11.
    ///
    /// A dry-run that does not say a password is coming has failed to answer the question it was
    /// run to answer.
    pub elevates: bool,
}

/// One thing an apply would do.
///
/// **The values a machine assigns at execution time are not here** (D8): the port a new instance
/// lands on, a generated password, a rowid. A plan that named them would be a plan the executor has
/// to contradict.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PlanAction {
    /// Register the directory as a project.
    RegisterProject {
        /// Its name.
        name: String,

        /// Its root.
        root: String,
    },

    /// Install a language runtime.
    InstallRuntime {
        /// Which language.
        kind: RuntimeKind,

        /// Which version, exactly as the blueprint pins it.
        version: PackageVersion,
    },

    /// Have a service instance, whether by reusing a shared one or by creating a dedicated one.
    EnsureService {
        /// The instance: `mariadb@main`, or `mariadb@shop` for one of the new project's own.
        service: ServiceId,

        /// The version the blueprint asks for, where it asks for one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<PackageVersion>,

        /// Whether this instance would belong to the new project alone.
        dedicated: bool,
    },

    /// Create a database and the account that reaches it.
    CreateDatabase {
        /// On which instance.
        service: ServiceId,

        /// The database name, `{project}` already expanded.
        database: String,

        /// The account name, `{project}` already expanded.
        ///
        /// **Never a password.** The manifest has no key for one, and what an apply generates is
        /// not something a plan can name in advance.
        user: String,
    },

    /// Create the site.
    CreateSite {
        /// What it serves.
        ///
        /// A php-fpm pool is not named here even though [`SiteKind::PhpFpm`] can carry one: which
        /// pool a new site uses is decided when the site is made, on the machine that makes it.
        kind: SiteKind,

        /// Relative to the project root; `""` is the root itself.
        doc_root: String,

        /// Whether HTTPS is declared.
        https: bool,
    },

    /// Give the site a name.
    AddDomain {
        /// The name, `{project}` already expanded.
        domain: String,

        /// Whether it is the primary.
        primary: bool,
    },

    /// Issue the site's certificate.
    IssueCertificate {
        /// Every name it would cover.
        domains: Vec<String>,
    },

    /// Turn a PHP extension on.
    ///
    /// **This reaches past the project.** Extension choices belong to an installed runtime, so
    /// enabling one changes the PHP that every project on the receiving machine runs — which is why
    /// the renderer says so on the line rather than in documentation nobody reads at that moment.
    ///
    /// There is no "off" direction (D2): a blueprint says what a project needs *loaded*, and
    /// disabling something for everybody else on the machine is harm it was never asked to do.
    SetPhpExtension {
        /// Which installed PHP.
        runtime: PackageVersion,

        /// The extension's name, as the index spells it.
        name: String,
    },

    /// Run the blueprint's `[scaffold]` command in the new project's directory.
    ///
    /// Capture never writes one. A hand-written or gallery blueprint may, and **T78a** is the task
    /// that decides whether it may run; here it only appears in the plan, as something a person
    /// would have to agree to.
    RunScaffold {
        /// The exact command, shown before anything runs it.
        command: String,
    },
}

/// What this home makes of one action.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Disposition {
    /// Already true. The apply does nothing here.
    Satisfied,

    /// It would be done.
    Create,

    /// A version mismatch, which is a question rather than a decision — T78 is what asks it.
    Choice {
        /// What this machine has.
        installed: PackageVersion,

        /// What the blueprint pins.
        wanted: PackageVersion,
    },

    /// Something a person has to agree to before it happens.
    Confirm {
        /// What they would be agreeing to.
        what: String,
    },

    /// It cannot be done.
    ///
    /// Decided **here** rather than five actions into an apply, which is the whole point of a plan
    /// (D10).
    Blocked {
        /// Why, in the words the CLI prints.
        reason: String,
    },

    /// This operating system cannot do it.
    Unsupported {
        /// Why.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The discriminator travels inside the object, so a client switches on one field — the shape
    /// [`crate::DaemonEvent`] and [`SiteKind`] already use, and the reason a variant added in a
    /// later phase arrives at an older client as an object it can ignore.
    #[test]
    fn an_action_and_its_disposition_are_flat_tagged_objects() {
        let step = PlanStep {
            action: PlanAction::InstallRuntime {
                kind: RuntimeKind::Php,
                version: PackageVersion::parse("8.2.23").expect("a version"),
            },
            disposition: Disposition::Choice {
                installed: PackageVersion::parse("8.2.29").expect("a version"),
                wanted: PackageVersion::parse("8.2.23").expect("a version"),
            },
            elevates: false,
        };

        let json = serde_json::to_value(&step).expect("a step encodes");

        assert_eq!(json["action"]["action"], "install_runtime");
        assert_eq!(json["action"]["version"], "8.2.23");
        assert_eq!(json["disposition"]["disposition"], "choice");
        assert_eq!(json["disposition"]["installed"], "8.2.29");

        let back: PlanStep = serde_json::from_value(json).expect("and decodes");
        assert_eq!(back, step);
    }

    /// A source is a closed word, because the column stores it and T78a's trust marking reads it.
    #[test]
    fn a_source_is_one_of_three_words_in_both_directions() {
        assert_eq!(
            serde_json::to_value(BlueprintSource::Captured).expect("encodes"),
            serde_json::json!("captured")
        );

        assert_eq!(
            BlueprintSource::parse("imported"),
            Some(BlueprintSource::Imported)
        );
        assert_eq!(BlueprintSource::parse("borrowed"), None);
    }

    /// A version the blueprint does not pin is left out rather than sent as `null`: an older client
    /// reading this reads *no version asked for*, which is what it means.
    #[test]
    fn a_service_without_a_pinned_version_carries_no_key_for_one() {
        let action = PlanAction::EnsureService {
            service: ServiceId::parse("redis@main").expect("an id"),
            version: None,
            dedicated: false,
        };

        let json = serde_json::to_value(&action).expect("it encodes");

        assert!(json.get("version").is_none(), "{json}");
    }
}
