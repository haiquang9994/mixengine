//! Making a database and the account that reaches it — roadmap task **T77a** — and handing it to
//! a desktop client — roadmap task **T83**.
//!
//! **What is not here is the password.** A response says *where* the credential is, in the address
//! the OS keyring holds it under, and never what it is: the T77a design's D11, and the same rule
//! [ADR 0006](../../../.claude/decisions/0006-servicespec-in-proto-and-secret-free.md) applies to a
//! [`ServiceSpec`](crate::ServiceSpec). T83's [`DatabaseHandoff`] keeps it: the password went into
//! the started process's environment, and what comes back is the same address.

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

/// Where a credential lives in this machine's credential store: the whole address, both halves.
///
/// **The convention two applications share** — roadmap task **T84**, the design's D6. Until that
/// task a response carried the key alone and the namespace lived inside `mixengine-platform`, so
/// anything outside this workspace that wanted to name the entry — MixDB, a graphical client — had
/// to hardcode the word `mixengine`. That second copy is what item 4 of
/// `features/extensions.md`'s MixDB list exists to remove.
///
/// A struct rather than one string with a separator: the two halves are two fields, so there is no
/// separator to pick and nothing to split wrong on the day a key holds the character somebody chose.
///
/// **Never the password.** An address is a name, which is why it is printed, logged and returned
/// freely while the value it points at is not.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SecretAddress {
    /// The application-side namespace: [`KEYRING_SERVICE`](crate::KEYRING_SERVICE), always.
    pub service: String,

    /// The account within it — `mariadb@main/root`, composed by
    /// `mixengine_core::services::handoff::secret_key`.
    pub key: String,
}

impl SecretAddress {
    /// The address of `key` inside MixEngine's own namespace.
    ///
    /// The only constructor, so nothing fills the namespace in by hand and no caller can put a
    /// different one there by accident.
    #[must_use]
    pub fn of(key: impl Into<String>) -> Self {
        Self {
            service: crate::KEYRING_SERVICE.to_owned(),
            key: key.into(),
        }
    }
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

    /// Where the account's password lives in this machine's credential store.
    ///
    /// **Never the password.** Derivable rather than secret — telling a caller the rule is what
    /// makes a second method for looking one up unnecessary until T83 gives that a shape.
    ///
    /// **Both halves since roadmap task T84**: the namespace is part of the contract, so a client
    /// renders the address without knowing any of MixEngine's constants.
    pub secret: SecretAddress,

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

    /// Where the administrator's password lives, for a server that has one — roadmap task **T84**.
    ///
    /// **Composed, never looked up.** This method starts nothing, opens nothing and touches the
    /// credential store not at all — T83's D6, unchanged — because the address is what the recipe
    /// and the service id say it is. That is what makes the convention *askable*: a client can draw
    /// "stored in your credential store as …" beside the button without opening a database to find
    /// out.
    ///
    /// [`None`] for a server with no accounts, and for a service no database client opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretAddress>,

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

    /// Where the password was read from. Never the password.
    ///
    /// Both halves since roadmap task **T84**, and the key half is also what the URL carries as
    /// `secret_key` — so the connection the client saves can point at this entry instead of holding
    /// a second copy of what is in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretAddress>,

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

    /// **Both halves, because a client that composes the other one is the duplication T84 removes**
    /// — the design's D6.
    #[test]
    fn an_address_carries_the_namespace_and_the_key() {
        let address = SecretAddress::of("mariadb@main/blog");

        assert_eq!(address.service, crate::KEYRING_SERVICE);
        assert_eq!(address.key, "mariadb@main/blog");

        let json = serde_json::to_value(&address).expect("it encodes");
        assert_eq!(json["service"], "mixengine");
        assert_eq!(json["key"], "mariadb@main/blog");

        let back: SecretAddress = serde_json::from_value(json).expect("and decodes");
        assert_eq!(back, address);
    }

    /// `database.client` says where the administrator's password is without opening anything, and
    /// says nothing at all for a server that has no accounts — the design's D6.
    #[test]
    fn a_client_report_names_the_address_for_a_server_with_accounts_and_none_otherwise() {
        let report = DatabaseClientReport {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            protocol: Some(DatabaseProtocol::Mysql),
            secret: Some(SecretAddress::of("mariadb@main/root")),
            client: DesktopClient::NoClient,
        };

        let json = serde_json::to_value(&report).expect("it encodes");
        assert_eq!(json["secret"]["service"], "mixengine");
        assert_eq!(json["secret"]["key"], "mariadb@main/root");

        let redis = DatabaseClientReport {
            service: ServiceId::parse("redis@main").expect("an id"),
            protocol: Some(DatabaseProtocol::Redis),
            secret: None,
            client: DesktopClient::NoClient,
        };

        let json = serde_json::to_value(&redis).expect("it encodes");
        assert!(
            json.get("secret").is_none(),
            "nothing to say is said by saying nothing: {json}"
        );
    }

    /// The response says what it did to each of the two objects, because that is what T78's ledger
    /// records: a rollback may only undo what its apply actually made.
    #[test]
    fn an_account_reports_what_was_made_and_where_the_password_is() {
        let answer = DatabaseAccount {
            service: ServiceId::parse("mariadb@main").expect("an id"),
            database: "blog".to_owned(),
            user: "blog".to_owned(),
            secret: SecretAddress::of("mariadb@main/blog"),
            made: Provisioned {
                database: Made::Created,
                user: Made::Existing,
            },
        };

        let json = serde_json::to_value(&answer).expect("it encodes");

        assert_eq!(json["made"]["database"], "created");
        assert_eq!(json["made"]["user"], "existing");
        assert_eq!(json["secret"]["service"], "mixengine");
        assert_eq!(json["secret"]["key"], "mariadb@main/blog");

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
            secret: SecretAddress::of("mariadb@main/blog"),
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
            secret: None,
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
            secret: Some(SecretAddress::of("mariadb@main/root")),
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
