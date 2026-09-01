//! Requests in the `blueprint.*` namespace — roadmap task **T77**.

use crate::{ProjectRef, RuntimeKind, ServiceId};

/// `blueprint.capture` — write down what a project is made of.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintCapture {
    /// Which project. The CLI fills this from the current directory when nobody named one.
    pub project: ProjectRef,

    /// The slug to file it under, which becomes the row's key and the rendered file's stem.
    pub name: String,

    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Whether to replace a blueprint already filed under this slug.
    ///
    /// Refusing by default is what stops a second capture quietly overwriting the first. There is
    /// no `blueprint.delete` in this build, so without this flag a mistyped name would be
    /// permanent — which is a worse default than asking.
    #[serde(default)]
    pub overwrite: bool,
}

/// `blueprint.apply` — what applying one would do, and (from T78) doing it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlueprintApply {
    /// Which blueprint, by slug.
    pub blueprint: String,

    /// What the new project is called, and what `{project}` expands to.
    pub project: String,

    /// Where it would live. Absolute; the client resolves it against its own current directory.
    pub root: String,

    /// Whether to stop after planning.
    ///
    /// **`false` is answered with `Unsupported` in this build**, naming T78 (the T77 design, D12).
    /// Not a `todo!()`, and not a CLI that refuses to send it: a client renders what the daemon
    /// answers rather than holding the rule itself.
    #[serde(default)]
    pub dry_run: bool,

    /// The answers to the version questions this plan raises — roadmap task **T78**.
    ///
    /// **A question is asked by a client and answered in the request**, because a daemon has no
    /// keyboard: a job that stopped halfway to ask one would be a job holding a project directory
    /// hostage. An apply meeting a [`Disposition::Choice`](crate::Disposition) with no answer here
    /// is refused before anything happens, naming every question it could not answer — and an
    /// answer to a question this plan does not ask is refused too, because it is an answer composed
    /// against a machine or a moment that has since changed.
    ///
    /// Defaulted, so a request written before this task still decodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub answers: Vec<VersionAnswer>,
}

/// One version question, answered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VersionAnswer {
    /// What the question was about.
    pub subject: AnswerSubject,

    /// What to do about it.
    pub answer: MismatchAnswer,
}

/// What a version question is about.
///
/// **Tagged rather than a bare string.** `php` is a language and `mariadb@main` is an instance; one
/// string field holding both is one collision away from applying an answer to the wrong thing.
///
/// A service question only ever arises for an instance that already exists, so its id is always
/// spellable — which is why this may carry a [`ServiceId`] where
/// [`PlanAction::EnsureService`](crate::PlanAction) deliberately carries the package and the
/// instance apart.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AnswerSubject {
    /// A language whose installed version is not the one the blueprint asks for.
    Runtime {
        /// Which language.
        kind: RuntimeKind,
    },

    /// A service instance already running a version the blueprint did not ask for.
    Service {
        /// Which instance.
        id: ServiceId,
    },
}

impl std::fmt::Display for AnswerSubject {
    /// The spelling a person typed, which is also the one a refusal names.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime { kind } => f.write_str(kind.as_str()),
            Self::Service { id } => f.write_str(id.as_str()),
        }
    }
}

/// What to do about a version that is not the one asked for.
///
/// **There is no `Cancel`.** The third answer in the feature doc's sentence needs nothing on the
/// wire: it is not sending the apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MismatchAnswer {
    /// Install what the blueprint asks for, and pin the project to it.
    ///
    /// Valid for a runtime. A service instance that already exists cannot be repointed at another
    /// version by this build — moving one is a database upgrade under somebody's data directory,
    /// and `service.create` and `service.delete` are the two ends of a row's life with nothing
    /// between them. The plan answers that combination with a `Blocked` step naming the way out.
    Install,

    /// Use what this machine has, and pin the project to *that* version.
    ///
    /// The pin is the whole point (the T78 design, D7): without it the two answers would produce
    /// identical machines and the question would be theatre.
    UseInstalled,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request from a client that has never heard of an answer still decodes: the field is
    /// defaulted, which is what stops this task breaking every caller T77 shipped.
    #[test]
    fn a_request_without_answers_still_decodes() {
        let asked: BlueprintApply = serde_json::from_value(serde_json::json!({
            "blueprint": "blog-stack",
            "project": "shop",
            "root": "/tmp/shop"
        }))
        .expect("a request older than this task");

        assert!(asked.answers.is_empty());
        assert!(!asked.dry_run);
    }

    /// **Two namespaces, told apart by a tag.** `php` is a language and `mariadb@main` is an
    /// instance, and one string field holding both is one collision away from applying an answer to
    /// the wrong thing.
    #[test]
    fn an_answer_names_which_kind_of_subject_it_is_about() {
        let answer = VersionAnswer {
            subject: AnswerSubject::Runtime {
                kind: RuntimeKind::Php,
            },
            answer: MismatchAnswer::UseInstalled,
        };

        let json = serde_json::to_value(&answer).expect("it encodes");
        assert_eq!(json["subject"]["subject"], "runtime");
        assert_eq!(json["subject"]["kind"], "php");
        assert_eq!(json["answer"], "use_installed");

        let back: VersionAnswer = serde_json::from_value(json).expect("and decodes");
        assert_eq!(back, answer);
    }

    /// The subject is what a refusal names, so it has one spelling and it is the one a person typed.
    #[test]
    fn a_subject_prints_as_the_thing_a_person_would_type() {
        assert_eq!(
            AnswerSubject::Runtime {
                kind: RuntimeKind::Php
            }
            .to_string(),
            "php"
        );
        assert_eq!(
            AnswerSubject::Service {
                id: ServiceId::parse("mariadb@main").expect("an id")
            }
            .to_string(),
            "mariadb@main"
        );
    }
}
