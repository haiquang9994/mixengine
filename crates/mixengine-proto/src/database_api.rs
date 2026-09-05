//! Requests in the `database.*` namespace — roadmap tasks **T77a** and **T83**.

use crate::ServiceId;

/// `database.client` — where one instance could be opened, and with what. Reads only.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DatabaseClientQuery {
    /// Which instance: `mariadb@main`, `redis@main`.
    pub service: ServiceId,
}

/// `database.open` — hand one instance to the installed desktop client.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DatabaseOpen {
    /// Which instance.
    pub service: ServiceId,

    /// The account to sign in as. The server's administrator when nobody says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// A database to open at, when the client should land in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// `database.create` — make sure a database and an account for it exist on one instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DatabaseCreate {
    /// Which instance: `mariadb@main`, `postgres@shop`.
    pub service: ServiceId,

    /// The database's name.
    pub database: String,

    /// The account's name. The database's own name when nobody says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}
