//! Making a database and an account on MariaDB or MySQL — roadmap task **T77a**.
//!
//! **One builder for two recipes**, because this is genuinely one syntax. The two servers differ in
//! how they are *bootstrapped* — `--bootstrap` against `--init-file`, which is why T33 wrote them
//! two rituals — and not in how a database is made. Two copies would be two places to fix a quoting
//! rule, and `mysql.rs` carries the test that would notice them drifting apart.
//!
//! # Every host a local client arrives as
//!
//! The generated `my.cnf` says `skip-name-resolve`, so a client connecting over TCP to 127.0.0.1 is
//! matched as `'blog'@'127.0.0.1'` and never as `'blog'@'localhost'` — T33's finding about the root
//! account, and just as true of this one. The account is created for all three of `localhost`,
//! `127.0.0.1` and `::1`, and granted on all three.
//!
//! # `CREATE` and then `ALTER`, and why the pair is safe
//!
//! `CREATE USER IF NOT EXISTS` does nothing to an account that is already there, so the `ALTER` is
//! what brings a server that has drifted back into agreement with the credential store. It is only
//! ever aimed at an account MixEngine holds the password for: the daemon refuses the other case
//! before these statements are built — the design's D3, *a keyring entry is the deed of ownership*.
//!
//! # No character set, and nothing escaped
//!
//! The instance is already configured with a `character_set` and a `collation`, so naming one here
//! would be a second place deciding it — one that silently wins (design D7). And every identifier is
//! quoted while nothing is escaped, which is safe because
//! [`validated_identifier`](crate::generate::databases::validated_identifier) refused every
//! character that could end a quoted one.

use std::path::PathBuf;

use mixengine_proto::Millis;

use crate::Result;
use crate::generate::databases::{Ask, Credentials, Found, password_env};
use crate::generate::recipe::Context;
use crate::generate::step::Step;

/// Hosts a client on this machine is matched as. See the module note.
const HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

/// The table the last step makes and drops to prove the account can write.
const CHECK_TABLE: &str = "mixengine_check";

/// How long any one of these gets.
///
/// A statement against a running server is milliseconds; this is the line past which the server is
/// not answering at all, which the caller wants reported rather than waited on.
const PATIENCE: Millis = Millis(30_000);

/// Everything about *reaching* this server that differs between the two recipes.
///
/// One value rather than four parameters: the recipes hold their own connection helpers — MariaDB's
/// client is `mariadb` and MySQL's is `mysql`, and each builds its own argument list — and passing
/// them apart made a function nobody could read the call site of.
pub(super) struct Client {
    /// The SQL client, absolute.
    pub(super) program: PathBuf,

    /// The environment variable its password arrives in.
    pub(super) variable: &'static str,

    /// The arguments that reach it as the superuser.
    pub(super) as_root: Vec<String>,

    /// The arguments that reach it as the account being made, against that account's database.
    pub(super) as_account: Vec<String>,
}

/// The read-only query that prints a word per object that exists.
///
/// `-N -B` is "no column names, batch": one bare word per line, which is what
/// [`Found::read`](crate::generate::databases::Found::read) reads.
///
/// # Errors
///
/// Whatever this instance cannot answer — an install publishing no client.
pub(super) fn probe(context: &Context, ask: &Ask, root: &str, client: &Client) -> Result<Step> {
    let mut args = client.as_root.clone();
    args.extend(["-N".to_owned(), "-B".to_owned()]);

    Ok(Step {
        label: format!(
            "look for the database {} and the account {}",
            ask.database, ask.user
        ),
        program: client.program.clone(),
        args,
        stdin: Some(format!(
            "SELECT 'database' FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '{}';\n\
             SELECT 'user' FROM mysql.user WHERE User = '{}' LIMIT 1;\n",
            ask.database, ask.user
        )),
        secret_file: None,
        env: password_env(client.variable, root),
        cwd: context.etc().to_path_buf(),
        timeout: PATIENCE,
    })
}

/// The statements, and last of all the login that proves they worked.
///
/// `found` is not read: every statement here is written to be true of a server in either state, which
/// is what makes a failed provisioning resumable by running it again. What the daemon does with
/// `found` is decide whether the account is *ours* — that decision is made before this is called,
/// and reported back to the caller as what was created.
///
/// # Errors
///
/// As [`probe`].
pub(super) fn steps(
    context: &Context,
    ask: &Ask,
    _found: Found,
    credentials: &Credentials,
    client: &Client,
) -> Result<Vec<Step>> {
    let (database, user) = (&ask.database, &ask.user);
    let password = &credentials.account;
    let mut sql = format!("CREATE DATABASE IF NOT EXISTS `{database}`;\n");

    for host in HOSTS {
        sql.push_str(&format!(
            "CREATE USER IF NOT EXISTS '{user}'@'{host}' IDENTIFIED BY '{password}';\n\
             ALTER USER '{user}'@'{host}' IDENTIFIED BY '{password}';\n\
             GRANT ALL PRIVILEGES ON `{database}`.* TO '{user}'@'{host}';\n"
        ));
    }

    sql.push_str("FLUSH PRIVILEGES;\n");

    let make = Step {
        label: format!("create the database {database} and the account {user}"),
        program: client.program.clone(),
        args: client.as_root.clone(),
        stdin: Some(sql),
        secret_file: None,
        env: password_env(client.variable, &credentials.root),
        cwd: context.etc().to_path_buf(),
        timeout: PATIENCE,
    };

    // **Design D13.** Everything above ran as the superuser and proves nothing about the account.
    // This runs *as the account*, against the database, and writes — so `database.create` cannot
    // answer with a success nobody can use.
    let check = Step {
        label: format!("log in as {user} and write to {database}"),
        program: client.program.clone(),
        args: client.as_account.clone(),
        stdin: Some(format!(
            "DROP TABLE IF EXISTS `{CHECK_TABLE}`;\n\
             CREATE TABLE `{CHECK_TABLE}` (one INT);\n\
             DROP TABLE `{CHECK_TABLE}`;\n"
        )),
        secret_file: None,
        env: password_env(client.variable, password),
        cwd: context.etc().to_path_buf(),
        timeout: PATIENCE,
    };

    Ok(vec![make, check])
}
