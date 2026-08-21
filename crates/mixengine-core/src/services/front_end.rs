//! Which service is already the home's front end — roadmap task **T37**.
//!
//! `.claude/features/services.md`: *exactly one of Caddy/Nginx is the active front end*. Until T37
//! there was one front-end recipe and the sentence could not be broken; with two, a home can hold a
//! `caddy` row and an `nginx` row, and what that is on the day sites arrive (T43) is two programs
//! generated against 80 and 443 with the second one failing to bind — or worse, succeeding, because
//! the first was stopped and half the sites are answered by a configuration nobody is looking at.
//!
//! **[`Instancing`] cannot say this and neither can a port.** Instancing is about how many rows may
//! name one package, and both front ends answer `Single`; the allocator would happily give the
//! second one 81, which is precisely the wrong kindness — a front end that has been renumbered is
//! not a front end. So the question is asked about the *role*
//! ([`Role::FrontEnd`](crate::generate::Role)) and answered where the row is written.
//!
//! [`Instancing`]: crate::generate::Instancing

use mixengine_proto::ServiceId;

use crate::generate::{Catalogue, Role};
use crate::{Result, Store};

/// The id of the service that is already this home's front end, or [`None`] when there is none.
///
/// **By what a package is for, not by what it is called.** The catalogue is what knows that `nginx`
/// and `caddy` are two answers to one question, and a home that acquires a third front end acquires
/// this refusal with it rather than a string added to a list here.
///
/// A row whose package this build has no recipe for is passed over rather than refused: a database
/// hand-edited or written by a later MixEngine is still a database, and what this function is for is
/// finding a front end — not auditing the table.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the table cannot be read.
pub async fn held_by(store: &Store, catalogue: &Catalogue) -> Result<Option<String>> {
    let rows = sqlx::query_scalar!("SELECT id FROM services")
        .fetch_all(store.pool())
        .await
        .map_err(|source| store.failure("read", source))?;

    Ok(rows.into_iter().find(|id| is_front_end(catalogue, id)))
}

/// Whether the service `id` names is one of the programs a site is reached through.
///
/// The package is the part of the id before the `@`, which is what every other caller of
/// [`ServiceId`] already relies on — and an id this build cannot parse belongs to no recipe here,
/// which is the same answer as a package it has no recipe for.
fn is_front_end(catalogue: &Catalogue, id: &str) -> bool {
    ServiceId::parse(id)
        .ok()
        .and_then(|service| catalogue.recipe(service.name()).map(|recipe| recipe.role()))
        .is_some_and(|role| role == Role::FrontEnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home with one service of each `package`, and the store it was written to.
    async fn home(packages: &[&str]) -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&directory.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");

        for package in packages {
            sqlx::query(
                "INSERT INTO packages (name, version, install_path, installed_at, source_url,
                                       sha256)
                 VALUES (?, '1.0.0', '/packages/x', '2026-08-21T00:00:00Z', 'https://example', 'ab')",
            )
            .bind(package)
            .execute(store.pool())
            .await
            .expect("a package for the service to belong to");

            sqlx::query(
                "INSERT INTO services (id, package_id, instance_name, state)
                 VALUES (?, (SELECT id FROM packages WHERE name = ?), 'main', 'stopped')",
            )
            .bind(package)
            .bind(package)
            .execute(store.pool())
            .await
            .expect("the service row");
        }

        (directory, store)
    }

    /// The one this call exists to find, and it is found by what it is *for*.
    ///
    /// Written with nginx rather than Caddy on purpose: the rule has to hold from either side, and a
    /// lookup that happened to be a comparison against the string `caddy` would pass the other way
    /// round and fail this one.
    #[tokio::test]
    async fn the_front_end_a_home_already_has_is_the_one_that_is_for_being_one() {
        let (_home, store) = home(&["mariadb", "nginx", "redis"]).await;

        assert_eq!(
            held_by(&store, &Catalogue::builtin())
                .await
                .expect("a readable table"),
            Some("nginx".to_owned())
        );
    }

    /// And the other one answers it too, which is the whole of what "exactly one of Caddy/Nginx"
    /// means: the rule is symmetric, so neither program may be the one the code happens to know
    /// about.
    #[tokio::test]
    async fn either_program_a_site_is_reached_through_answers_to_being_the_front_end() {
        let (_home, store) = home(&["caddy", "postgres"]).await;

        assert_eq!(
            held_by(&store, &Catalogue::builtin())
                .await
                .expect("a readable table"),
            Some("caddy".to_owned())
        );
    }

    /// And a home full of servers that are not front ends has none, which is what makes the refusal
    /// this feeds a refusal about front ends rather than about services.
    #[tokio::test]
    async fn a_home_whose_services_are_all_something_else_holds_none() {
        let (_home, store) = home(&["mariadb", "redis", "memcached"]).await;

        assert_eq!(
            held_by(&store, &Catalogue::builtin())
                .await
                .expect("a readable table"),
            None
        );
    }
}
