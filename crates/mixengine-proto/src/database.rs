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

/// What a database client speaks to one of our servers — roadmap task **T83**, the design's D5.
///
/// **The recipe's answer, and a fact about the server.** MariaDB speaks MySQL's protocol whichever
/// client is on the other end, so `mariadb` and `mysql` are one word here. The word is what the
/// handoff URL carries as `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseProtocol {
    /// MySQL's wire protocol: MariaDB and MySQL.
    Mysql,

    /// PostgreSQL's.
    Postgres,

    /// RESP — Redis. No accounts: the recipe sets no password, so a handoff carries no credential.
    Redis,
}

impl DatabaseProtocol {
    /// The word in the URL, which is the word on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Postgres => "postgres",
            Self::Redis => "redis",
        }
    }

    /// Whether this server has accounts to sign in as. `false` for Redis, where `--user` is refused.
    #[must_use]
    pub const fn has_accounts(self) -> bool {
        !matches!(self, Self::Redis)
    }
}

impl std::fmt::Display for DatabaseProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a database could be opened, as a state — roadmap task **T83**, the design's D3.
///
/// **Three answers, and two of them are not errors.** A client renders `not_installed` and
/// `no_client` as an absent affordance with a sentence beside it; an error would be rendered as a
/// failure of something the person did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DesktopClient {
    /// A `desktop-app` extension is installed and this machine has the application.
    Installed {
        /// The extension.
        extension: crate::ExtensionId,
        /// Its display name — `MixDB`.
        name: String,
        /// The executable this machine would start.
        program: String,
    },

    /// The extension is installed; the application is not on this machine.
    NotInstalled {
        /// The extension.
        extension: crate::ExtensionId,
        /// Its display name.
        name: String,
        /// Where this system looked, phrased for a person.
        searched: String,
        /// Where to get it, when the manifest says.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        homepage: Option<String>,
    },

    /// No `desktop-app` extension is installed here.
    NoClient,
}

/// What became of the process — roadmap task **T83**, the design's D8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "launch", rename_all = "snake_case")]
pub enum Launch {
    /// Still running a second after it was started.
    Running {
        /// Its process id.
        pid: u32,
    },

    /// Exited successfully within that second — a client that handed the connection to an instance
    /// already running, which is what every single-instance desktop application does.
    HandedOn,
}

/// `database.client`'s answer: whether one service could be opened, and with what.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DatabaseClientReport {
    /// Which instance.
    pub service: ServiceId,

    /// What a client would speak to it, or [`None`] for a service no database client opens —
    /// memcached, a front end, a pool. A state, not a refusal: `database.open` is what refuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<DatabaseProtocol>,

    /// Where it could be opened.
    pub client: DesktopClient,
}

/// `database.open`'s answer.
///
/// **What is not here is the password**, exactly as [`DatabaseAccount`]: `secret` is the keyring
/// address it was read from, and the value went into one process's environment and nowhere else.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DatabaseHandoff {
    /// Which instance.
    pub service: ServiceId,

    /// What the client was told to speak.
    pub protocol: DatabaseProtocol,

    /// The account it was signed in as. [`None`] for a server with no accounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// The database it was pointed at, when one was named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// Where the password was read from: `<service-id>/<user>`. Never the password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,

    /// The client, as it was found.
    pub client: DesktopClient,

    /// What was started — present exactly when `client` is `installed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launched: Option<Launch>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DatabaseCreate, DatabaseOpen};

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

    /// The three states a client can be in are three words on the wire, and a person can read them.
    #[test]
    fn a_client_state_is_tagged_by_its_state() {
        let report = DatabaseClientReport {
            service: ServiceId::parse("redis@main").expect("an id"),
            protocol: Some(DatabaseProtocol::Redis),
            client: DesktopClient::NotInstalled {
                extension: crate::ExtensionId::parse("mixdb").expect("an id"),
                name: "MixDB".to_owned(),
                searched: "App Paths and the uninstall table".to_owned(),
                homepage: Some("https://github.com/mixnz/mixdb".to_owned()),
            },
        };

        let json = serde_json::to_value(&report).expect("it encodes");
        assert_eq!(json["protocol"], "redis");
        assert_eq!(json["client"]["state"], "not_installed");
        assert_eq!(
            json["client"]["searched"],
            "App Paths and the uninstall table"
        );

        let back: DatabaseClientReport = serde_json::from_value(json).expect("and decodes");
        assert_eq!(back, report);

        let none = serde_json::to_value(DesktopClient::NoClient).expect("it encodes");
        assert_eq!(none["state"], "no_client");
    }

    /// A handoff carries where the password was read from and never the password — D2, and
    /// [`DatabaseAccount`]'s rule at the next address.
    #[test]
    fn a_handoff_names_the_keyring_address_and_nothing_shaped_like_a_password() {
        let handoff = DatabaseHandoff {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            protocol: DatabaseProtocol::Mysql,
            user: Some("root".to_owned()),
            database: None,
            secret: Some("mariadb@main/root".to_owned()),
            client: DesktopClient::Installed {
                extension: crate::ExtensionId::parse("mixdb").expect("an id"),
                name: "MixDB".to_owned(),
                program: "/Applications/MixDB.app/Contents/MacOS/mixdb".to_owned(),
            },
            launched: Some(Launch::Running { pid: 4242 }),
        };

        let rendered = serde_json::to_string(&handoff).expect("it encodes");
        assert!(!rendered.contains("password"), "{rendered}");

        let json: serde_json::Value = serde_json::from_str(&rendered).expect("json");
        assert_eq!(json["launched"]["launch"], "running");
        assert_eq!(json["launched"]["pid"], 4242);
        assert_eq!(json["client"]["state"], "installed");
        assert!(json.get("database").is_none(), "{json}");

        let handed = serde_json::to_value(Launch::HandedOn).expect("it encodes");
        assert_eq!(handed["launch"], "handed_on");
    }

    /// The word in the URL is the word on the wire.
    #[test]
    fn a_protocol_spells_itself_the_same_way_everywhere() {
        for (protocol, word) in [
            (DatabaseProtocol::Mysql, "mysql"),
            (DatabaseProtocol::Postgres, "postgres"),
            (DatabaseProtocol::Redis, "redis"),
        ] {
            assert_eq!(protocol.as_str(), word);
            assert_eq!(serde_json::to_value(protocol).expect("encodes"), word);
        }
        assert!(DatabaseProtocol::Mysql.has_accounts());
        assert!(!DatabaseProtocol::Redis.has_accounts());
    }

    /// An open with nothing but a service carries nothing but a service.
    #[test]
    fn an_open_without_an_account_carries_no_key_for_one() {
        let asked = DatabaseOpen {
            service: ServiceId::parse("postgres@main").expect("an id"),
            user: None,
            database: None,
        };
        let json = serde_json::to_value(&asked).expect("it encodes");
        assert!(
            json.get("user").is_none() && json.get("database").is_none(),
            "{json}"
        );
    }
}
