//! Making a database and the account that reaches it — roadmap task **T77a**.
//!
//! **What is not here is the password.** A response says *where* the credential is, in the address
//! the OS keyring holds it under, and never what it is: the T77a design's D11, and the same rule
//! [ADR 0006](../../../.claude/decisions/0006-servicespec-in-proto-and-secret-free.md) applies to a
//! [`ServiceSpec`](crate::ServiceSpec). Handing a credential to a program that needs one is T83's
//! design, and a second shape for it here would be one T83 has to contradict.

use crate::ServiceId;

/// What one call did to one object.
///
/// Two words rather than a `bool`, and they are read by more than a renderer: **T78's ledger
/// records this**, because a rollback may only undo what that apply actually created — and
/// `true`/`false` on a field called `created` reads the same whichever way somebody wires it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Made {
    /// This call made it.
    Created,

    /// It was already there, and this call left it as it found it.
    Existing,
}

/// What became of the database, and of the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Provisioned {
    /// The database.
    pub database: Made,

    /// The account.
    pub user: Made,
}

/// A database and the account that reaches it, as `database.create` answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DatabaseAccount {
    /// Which instance holds it.
    pub service: ServiceId,

    /// The database's name.
    pub database: String,

    /// The account's name.
    pub user: String,

    /// Where the account's password lives in the OS keyring: `<service-id>/<user>`.
    ///
    /// **Never the password.** Derivable rather than secret — telling a caller the rule is what
    /// makes a second method for looking one up unnecessary until T83 gives that a shape.
    pub secret: String,

    /// What this call made, and what it found already there.
    pub made: Provisioned,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatabaseCreate;

    /// The response says what it did to each of the two objects, because that is what T78's ledger
    /// records: a rollback may only undo what its apply actually made.
    #[test]
    fn an_account_reports_what_was_made_and_where_the_password_is() {
        let answer = DatabaseAccount {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            database: "blog".to_owned(),
            user: "blog".to_owned(),
            secret: "mariadb@main/blog".to_owned(),
            made: Provisioned {
                database: Made::Created,
                user: Made::Existing,
            },
        };

        let json = serde_json::to_value(&answer).expect("it encodes");

        assert_eq!(json["made"]["database"], "created");
        assert_eq!(json["made"]["user"], "existing");
        assert_eq!(json["secret"], "mariadb@main/blog");

        let back: DatabaseAccount = serde_json::from_value(json).expect("and decodes");
        assert_eq!(back, answer);
    }

    /// **The password is not a field, and this test says so on purpose.** D11: the wire carries the
    /// address of a credential and never the credential — a response that grew one would put it in
    /// `daemon.log`, in a `--json` pipeline and in a shell history.
    #[test]
    fn nothing_on_the_wire_is_shaped_like_a_password() {
        let rendered = serde_json::to_string(&DatabaseAccount {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            database: "blog".to_owned(),
            user: "blog".to_owned(),
            secret: "mariadb@main/blog".to_owned(),
            made: Provisioned {
                database: Made::Created,
                user: Made::Created,
            },
        })
        .expect("it encodes");

        assert!(!rendered.contains("password"), "{rendered}");
    }

    /// An account nobody named is the database's own name, and the request says so by leaving the
    /// key out rather than sending `null` — an older client reads *nobody said*, which is what it
    /// means.
    #[test]
    fn a_request_without_an_account_carries_no_key_for_one() {
        let asked = DatabaseCreate {
            service: ServiceId::parse("postgres@main").expect("an id"),
            database: "shop".to_owned(),
            user: None,
        };

        let json = serde_json::to_value(&asked).expect("it encodes");

        assert!(json.get("user").is_none(), "{json}");
    }
}
