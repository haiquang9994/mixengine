//! Handing one database service to a desktop client — roadmap task **T83**.
//!
//! Two pure things and one read. [`address`] joins a `services` row to its recipe the way
//! [`crate::extensions::database::endpoint`] does, and disagrees with it on purpose about Redis:
//! phpMyAdmin cannot administer a cache, and MixDB can open one. [`url`] spells the connection the
//! way the client reads it, and [`encode`] is the ten lines that keep a crate out of the tree for
//! one function.
//!
//! **No password anywhere in this module.** The URL names the variable the password travels in
//! (the design's D2) and the daemon fills that variable; nothing here sees the value.

use std::fmt::Write as _;
use std::net::IpAddr;

use mixengine_proto::{DatabaseProtocol, ServiceId};

use crate::generate::Catalogue;
use crate::{Error, Result, Store};

/// The environment variable a credential is handed over in.
///
/// T82a's name, re-exported rather than restated: one constant, two consumers.
pub use crate::extensions::pools::CREDENTIAL_ENV;

/// Where one account's password lives inside the keyring's
/// [`mixengine`](mixengine_proto::KEYRING_SERVICE) namespace.
///
/// `<service-id>/<user>` — `mariadb@main/root`. The service id rather than the package name,
/// because two instances of one server are two databases with two different passwords.
///
/// **One composition, and roadmap task T84 is why it is here rather than in three places.** Until
/// this task the string was spelled by
/// [`Context::secret_address`](crate::generate::recipe::Context::secret_address) for the recipes,
/// again by `database.open` for the handoff, and read back by the daemon's credential reader. The
/// convention is now published to another application — MixDB reads these entries — and a rule that
/// two `format!`s agree on by inspection is a rule that drifts the first time one of them moves.
#[must_use]
pub fn secret_key(service: &ServiceId, user: &str) -> String {
    format!("{}/{user}", service.as_str())
}

/// Where one database service listens, and what it speaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// What a client speaks to it.
    pub protocol: DatabaseProtocol,

    /// The address the row binds.
    pub host: IpAddr,

    /// The port the row declares.
    pub port: u16,

    /// The recipe's administrator — `root`, `postgres` — or [`None`] for a server with no accounts.
    pub administrator: Option<String>,
}

/// What a URL is rendered from.
#[derive(Debug, Clone, Copy)]
pub struct Connection<'a> {
    /// The client's scheme, out of its manifest: `mixdb`.
    pub scheme: &'a str,

    /// The label the client names the tab with: the service id.
    pub label: &'a str,

    /// Where to connect.
    pub address: &'a Address,

    /// The account to sign in as, where the server has accounts.
    pub user: Option<&'a str>,

    /// A database to land in.
    pub database: Option<&'a str>,
}

/// The address of one service, or [`None`] for a service no database client opens.
///
/// A row with no port is answered the same way: every database recipe binds a TCP port on all three
/// systems (T34c), so a row without one is a service nothing can dial.
///
/// # Errors
///
/// [`Error::NotFound`] when there is no such row, [`Error::Database`] when it cannot be read.
pub async fn address(store: &Store, service: &ServiceId) -> Result<Option<Address>> {
    let id = service.as_str();

    let row = sqlx::query!(
        "SELECT p.name AS package, s.port, s.bind_addr
         FROM services s
         JOIN packages p ON p.id = s.package_id
         WHERE s.id = ?",
        id
    )
    .fetch_optional(store.pool())
    .await
    .map_err(|source| store.failure("read", source))?
    .ok_or_else(|| Error::NotFound {
        kind: "service",
        id: id.to_owned(),
    })?;

    let catalogue = Catalogue::builtin();
    let Some(recipe) = catalogue.recipe(&row.package) else {
        return Ok(None);
    };
    let (Some(protocol), Some(port)) = (recipe.protocol(), row.port) else {
        return Ok(None);
    };

    Ok(Some(Address {
        protocol,
        host: crate::services::ports::bind_address(Some(row.bind_addr.as_str())),
        // The column is an `INTEGER`; a value outside a port's range is a row nothing wrote.
        port: u16::try_from(port).unwrap_or_default(),
        administrator: recipe.administrator().map(str::to_owned),
    }))
}

/// The URL the client is started with.
///
/// `<scheme>://connect?kind=…&host=…&port=…[&user=…][&database=…]&label=…[&password_env=…]`.
/// `password_env` is present exactly when `user` is: it says where the password is, and there is
/// none to say anything about for a server with no accounts.
#[must_use]
pub fn url(connection: &Connection<'_>) -> String {
    let address = connection.address;
    let mut rendered = format!(
        "{}://connect?kind={}&host={}&port={}",
        connection.scheme,
        address.protocol.as_str(),
        encode(&address.host.to_string()),
        address.port
    );

    if let Some(user) = connection.user {
        let _ = write!(rendered, "&user={}", encode(user));
    }
    if let Some(database) = connection.database {
        let _ = write!(rendered, "&database={}", encode(database));
    }
    let _ = write!(rendered, "&label={}", encode(connection.label));
    if connection.user.is_some() {
        let _ = write!(rendered, "&password_env={CREDENTIAL_ENV}");
    }

    rendered
}

/// Percent-encode everything outside RFC 3986's unreserved set.
#[must_use]
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address_of(protocol: DatabaseProtocol, administrator: Option<&str>) -> Address {
        Address {
            protocol,
            host: IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 3306,
            administrator: administrator.map(str::to_owned),
        }
    }

    /// **One composition, published to another application** — roadmap task **T84**, the design's
    /// D6. The address MixDB is told to read and the address MixEngine writes are the same string
    /// because they are the same function, not because two `format!`s agree by inspection.
    #[test]
    fn the_key_a_recipe_writes_and_the_key_a_handoff_names_are_one_function() {
        let service = ServiceId::parse("mariadb@main").expect("an id");

        assert_eq!(secret_key(&service, "root"), "mariadb@main/root");
        assert_eq!(secret_key(&service, "blog"), "mariadb@main/blog");
    }

    /// Every recipe answers, and only the databases say a word.
    #[test]
    fn the_databases_speak_a_protocol_and_nothing_else_does() {
        let catalogue = Catalogue::builtin();
        let says = |package: &str| catalogue.recipe(package).expect("a recipe").protocol();

        assert_eq!(says("mariadb"), Some(DatabaseProtocol::Mysql));
        assert_eq!(says("mysql"), Some(DatabaseProtocol::Mysql));
        assert_eq!(says("postgres"), Some(DatabaseProtocol::Postgres));
        assert_eq!(says("redis"), Some(DatabaseProtocol::Redis));
        assert_eq!(says("memcached"), None);
        assert_eq!(says("caddy"), None);
        assert_eq!(says("php-fpm"), None);
    }

    /// The whole shape, with every optional part present.
    #[test]
    fn a_url_carries_the_address_the_account_and_the_variable_name() {
        let address = address_of(DatabaseProtocol::Mysql, Some("root"));
        let rendered = url(&Connection {
            scheme: "mixdb",
            label: "mariadb@main",
            address: &address,
            user: Some("blog"),
            database: Some("blog"),
        });

        assert_eq!(
            rendered,
            "mixdb://connect?kind=mysql&host=127.0.0.1&port=3306&user=blog&database=blog\
             &label=mariadb%40main&password_env=MIXENGINE_DB_PASSWORD"
        );
    }

    /// A server with no accounts hands over an address and a label, and names no variable — there
    /// is nothing in the environment for the client to read.
    #[test]
    fn a_redis_url_names_no_account_and_no_variable() {
        let mut address = address_of(DatabaseProtocol::Redis, None);
        address.port = 6379;
        let rendered = url(&Connection {
            scheme: "mixdb",
            label: "redis@main",
            address: &address,
            user: None,
            database: None,
        });

        assert_eq!(
            rendered,
            "mixdb://connect?kind=redis&host=127.0.0.1&port=6379&label=redis%40main"
        );
        assert!(!rendered.contains("password"), "{rendered}");
    }

    /// Everything outside the unreserved set is escaped, and the unreserved set is left alone.
    #[test]
    fn encoding_escapes_what_a_query_string_cannot_carry() {
        assert_eq!(encode("mariadb@main"), "mariadb%40main");
        assert_eq!(encode("a b&c=d/e"), "a%20b%26c%3Dd%2Fe");
        assert_eq!(encode("plain-name_1.0~x"), "plain-name_1.0~x");
        assert_eq!(encode("é"), "%C3%A9");
    }

    /// The row and the recipe together, over a real store: the address of a database, a cache a
    /// client opens, a cache nothing opens, and a service that is not there.
    #[tokio::test]
    async fn an_address_is_read_off_the_row_and_the_recipe() {
        let (_temp, store) = home().await;
        a_service(&store, "mariadb@main", "mariadb", 3307).await;
        a_service(&store, "redis@main", "redis", 6379).await;
        a_service(&store, "memcached@main", "memcached", 11211).await;

        let mariadb = address(&store, &ServiceId::parse("mariadb@main").expect("an id"))
            .await
            .expect("it reads")
            .expect("a database");
        assert_eq!(mariadb.protocol, DatabaseProtocol::Mysql);
        assert_eq!(mariadb.port, 3307);
        assert_eq!(mariadb.administrator.as_deref(), Some("root"));

        let redis = address(&store, &ServiceId::parse("redis@main").expect("an id"))
            .await
            .expect("it reads")
            .expect("a cache a client opens");
        assert_eq!(redis.protocol, DatabaseProtocol::Redis);
        assert_eq!(redis.administrator, None);

        assert!(
            address(&store, &ServiceId::parse("memcached@main").expect("an id"))
                .await
                .expect("it reads")
                .is_none()
        );

        let missing = address(&store, &ServiceId::parse("postgres@main").expect("an id"))
            .await
            .expect_err("no such row");
        assert!(matches!(missing, Error::NotFound { .. }), "{missing}");
    }

    /// An empty home with the migrations applied.
    async fn home() -> (tempfile::TempDir, Store) {
        let temp = tempfile::tempdir().expect("a temporary home");
        let store = Store::open(&temp.path().join("mixengine.db"))
            .await
            .expect("a store");
        (temp, store)
    }

    /// A `packages` row and the `services` row that runs out of it.
    async fn a_service(store: &Store, service: &str, package: &str, port: i64) {
        let instance = service.split('@').nth(1).unwrap_or("main");

        let package_id = sqlx::query_scalar!(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/x', '2026-09-03T00:00:00Z',
                     'https://example.invalid/x.zip', 'ab')
             ON CONFLICT (name, version) DO UPDATE SET name = excluded.name
             RETURNING id",
            package
        )
        .fetch_one(store.pool())
        .await
        .expect("a package row");

        sqlx::query!(
            "INSERT INTO services (id, package_id, instance_name, state, port, bind_addr)
             VALUES (?, ?, ?, 'stopped', ?, '127.0.0.1')",
            service,
            package_id,
            instance,
            port
        )
        .execute(store.pool())
        .await
        .expect("a service row");
    }
}
