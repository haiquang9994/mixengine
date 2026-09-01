//! How a package administers its databases: what to ask, and what to run — roadmap task **T77a**.
//!
//! **The recipe declares, the daemon performs** — T33's division, one table across (the T77a design,
//! D1). A recipe lives here, in a crate with no business reaching an OS credential store and no
//! business spawning anything, so what it answers is *which statements*; the daemon answers *with
//! which credential, against which running server*.
//!
//! # The probe, and why its output is two words
//!
//! The daemon has to know which of the two objects already exist before it can decide anything, and
//! the obvious shape — each recipe emitting its own output and the daemon parsing it — would put
//! MariaDB's result format in the daemon and undo D1. So each engine's query is written to print the
//! word `database` on a line if the database is there, and `user` on a line if the account is. Three
//! queries, one [`Found::read`].
//!
//! # Validated, never escaped
//!
//! Every statement quotes its identifiers and nothing escapes them, which is safe for exactly one
//! reason: [`validated_identifier`] refuses every character that could end a quoted identifier. There
//! is no escaping function in this crate to get wrong, and nothing reaches a statement that it did
//! not accept.

use std::collections::BTreeMap;

use super::recipe::Context;
use super::step::Step;
use crate::{Error, Result};

/// The longest a database or account name may be.
///
/// MySQL's limit on an account name, which is the shorter of the two limits and therefore the only
/// one worth having: a name a database would accept and an account would not is a `CREATE USER` the
/// server refuses *after* the database has been made. The same number is `DATABASE_USER_LIMIT` in
/// [`crate::blueprints::plan`], where T77 refuses it a whole apply earlier.
pub const IDENTIFIER_LIMIT: usize = 32;

/// How many characters a generated account password has.
///
/// The length the databases' own superuser credentials are given in [`super::recipes`], for the same
/// reason: long enough that nothing on this machine is guessing it, short enough that every client
/// takes it as one word.
pub const SECRET_LENGTH: usize = 32;

/// The word a probe prints for a database that is there.
const DATABASE_WORD: &str = "database";

/// The word a probe prints for an account that is there.
const USER_WORD: &str = "user";

/// What was asked for, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    /// The database's name.
    pub database: String,

    /// The account's name.
    pub user: String,
}

/// The two passwords a set of steps interpolates.
pub struct Credentials {
    /// The superuser's, read out of the keyring by the daemon.
    pub root: String,

    /// The account's — already stored, or generated a moment ago.
    pub account: String,
}

/// Written by hand, and both fields are the reason.
///
/// [`Step`] redacts what it carries for this rule, and this type is nothing *but* the thing it
/// redacts: `.claude/standards/rust.md` says a struct which might hold a secret redacts it rather
/// than trusting every caller that ever writes `{:?}`, and a `tracing` field on a provisioning that
/// failed is one line away at all times. Not "no `Debug` at all", because that only moves the
/// question to whoever puts this inside something else.
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("root", &format!("<{} bytes>", self.root.len()))
            .field("account", &format!("<{} bytes>", self.account.len()))
            .finish()
    }
}

/// What the probe found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Found {
    /// Whether the database is there.
    pub database: bool,

    /// Whether the account is there.
    pub user: bool,
}

impl Found {
    /// Read a probe's standard output.
    ///
    /// Lines rather than a format: `psql -tA` prints a bare word and `mariadb -N -B` prints a bare
    /// word, and either may put a blank line or padding around it. A line this build does not
    /// recognise says nothing — a client's warning on standard output must not read as an object
    /// that exists.
    #[must_use]
    pub fn read(output: &str) -> Self {
        let mut found = Self::default();

        for line in output.lines().map(str::trim) {
            match line {
                DATABASE_WORD => found.database = true,
                USER_WORD => found.user = true,
                _ => {}
            }
        }

        found
    }
}

/// How one package administers its databases, as a recipe declares it.
///
/// Function pointers rather than more trait methods, on [`super::first_run::Ritual`]'s reasoning:
/// the declaration and the thing it declares are one value, so a recipe cannot name a superuser
/// credential and then have no statements to use it in.
#[derive(Debug, Clone, Copy)]
pub struct DatabaseAdmin {
    /// The key this package's superuser password is stored under — [`Context::secret_address`]'s
    /// argument. `root` for the MySQL family, `postgres` for PostgreSQL: the account differs, so the
    /// address does.
    pub root: &'static str,

    /// The read-only query that prints a word per object that exists.
    pub probe: fn(&Context, &Ask, &str) -> Result<Step>,

    /// The statements that make what is missing, bring the account's password into line, grant it,
    /// and — last — log in *as that account* and write with it (design D13).
    pub steps: fn(&Context, &Ask, Found, &Credentials) -> Result<Vec<Step>>,
}

/// Hold a name to what both a database and an account can be called.
///
/// See the module note: this refusal is what makes quoting sufficient and escaping unnecessary.
///
/// # Errors
///
/// [`Error::InvalidDatabaseName`] naming the rule that was broken, in the words the user is shown.
pub fn validated_identifier(name: &str) -> Result<String> {
    let refuse = |reason: &'static str| {
        Err(Error::InvalidDatabaseName {
            name: name.to_owned(),
            reason,
        })
    };

    if name.is_empty() {
        return refuse("it is empty");
    }
    if name.chars().count() > IDENTIFIER_LIMIT {
        return refuse(
            "it is longer than thirty-two characters, which is the longest account name MySQL \
             accepts",
        );
    }
    if !name.chars().all(|character| {
        character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
    }) {
        return refuse("only lower-case letters, digits and hyphens are allowed");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return refuse("it starts or ends with a hyphen");
    }

    Ok(name.to_owned())
}

/// A [`DatabaseAdmin`], bound to the instance it is for.
///
/// Holds the [`Context`] for [`super::first_run::FirstRun`]'s reason: the steps cannot be built
/// until the daemon has the credentials, and the daemon should not read a credential until it knows
/// there is a vocabulary to use it with.
#[derive(Debug, Clone)]
pub struct Provisioning {
    admin: DatabaseAdmin,
    context: Context,
}

impl Provisioning {
    /// The vocabulary `admin` declares, for the service `context` describes.
    pub(super) fn new(context: &Context, admin: DatabaseAdmin) -> Self {
        Self {
            admin,
            context: context.clone(),
        }
    }

    /// Where this instance's superuser password lives in the OS keyring.
    #[must_use]
    pub fn root_address(&self) -> String {
        self.context.secret_address(self.admin.root)
    }

    /// Where an account's password lives, or would.
    #[must_use]
    pub fn secret_address(&self, user: &str) -> String {
        self.context.secret_address(user)
    }

    /// The query that says what is already there.
    ///
    /// # Errors
    ///
    /// Whatever this instance cannot answer — a row carrying no port, an install that publishes no
    /// client.
    pub fn probe(&self, ask: &Ask, root: &str) -> Result<Step> {
        (self.admin.probe)(&self.context, ask, root)
    }

    /// The statements, now that both credentials exist.
    ///
    /// # Errors
    ///
    /// As [`probe`](Self::probe).
    pub fn steps(&self, ask: &Ask, found: Found, credentials: &Credentials) -> Result<Vec<Step>> {
        (self.admin.steps)(&self.context, ask, found, credentials)
    }
}

/// The environment a client is run with, over the platform's own floor.
///
/// One function rather than a literal in three recipes: the variable's *name* differs per engine and
/// the arrangement does not — the password reaches the client through the environment and never
/// through `args`, exactly as the health checks already reach it.
#[must_use]
pub fn password_env(variable: &str, password: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(variable.to_owned(), password.to_owned())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one reading of a probe's output, and the reason each engine's *query* is what differs: a
    /// per-engine parser would put MariaDB's output format in the daemon — design D4.
    #[test]
    fn a_probe_reports_by_printing_a_word_for_each_thing_that_exists() {
        assert_eq!(
            Found::read(""),
            Found {
                database: false,
                user: false
            }
        );
        assert_eq!(
            Found::read("database\n"),
            Found {
                database: true,
                user: false
            }
        );
        assert_eq!(
            Found::read("user\n"),
            Found {
                database: false,
                user: true
            }
        );
        assert_eq!(
            Found::read("database\nuser\n"),
            Found {
                database: true,
                user: true
            }
        );
    }

    /// psql pads and the MySQL client does not; a line of either shape means the same thing.
    #[test]
    fn surrounding_space_and_blank_lines_are_not_a_third_answer() {
        assert_eq!(
            Found::read("\n  database  \n\n user\n"),
            Found {
                database: true,
                user: true
            }
        );
    }

    /// Anything the query did not promise to print is not an answer.
    #[test]
    fn a_word_this_build_does_not_know_says_nothing() {
        assert_eq!(
            Found::read("Warning: using a password on the command line\n"),
            Found::default()
        );
    }

    /// **Validated, never escaped.** The characters refused here are exactly the ones that would end
    /// a quoted identifier early, which is what makes every statement's quoting sufficient.
    #[test]
    fn an_identifier_is_a_slug_and_nothing_else() {
        assert_eq!(validated_identifier("blog").ok().as_deref(), Some("blog"));
        assert_eq!(
            validated_identifier("my-blog").ok().as_deref(),
            Some("my-blog")
        );

        for refused in [
            "",
            "-blog",
            "blog-",
            "Blog",
            "blog;drop",
            "blog`x",
            "blog'x",
            "blog\"x",
            "blog x",
            "blog\\x",
        ] {
            assert!(
                matches!(
                    validated_identifier(refused),
                    Err(Error::InvalidDatabaseName { .. })
                ),
                "{refused:?} was accepted"
            );
        }
    }

    /// MySQL refuses an account name longer than this, and finding that out from the server is
    /// finding it out after the database has been made.
    #[test]
    fn an_identifier_stops_at_the_length_an_account_may_have() {
        assert!(validated_identifier(&"a".repeat(IDENTIFIER_LIMIT)).is_ok());
        assert!(validated_identifier(&"a".repeat(IDENTIFIER_LIMIT + 1)).is_err());
    }
}
