//! Requests in the `database.*` namespace — roadmap task **T77a**.

use crate::ServiceId;

/// `database.create` — make sure a database and an account for it exist on one instance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DatabaseCreate {
    /// Which instance: `mariadb@main`, `postgres@shop`.
    pub service: ServiceId,

    /// The database's name.
    pub database: String,

    /// The account's name. The database's own name when nobody says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}
