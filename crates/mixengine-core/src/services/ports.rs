//! Which port a new service is given — roadmap task **T34c**.
//!
//! MariaDB and MySQL name the same default, and so do two instances of either, which is one problem
//! and not two. `.claude/features/services.md` answers it in one rule and this module is that rule:
//! **a port is allocated once, when the row is written, and never computed again.** What a recipe
//! names is a wish ([`Recipe::preferred_port`]); the first row to ask for it is given it, and the
//! next is given the first free port above.
//!
//! Three things it has to get right, and each of them is a test below.
//!
//! **Free means free on the machine, not free in the table.** 3306 on a developer's machine is
//! routinely held by an XAMPP, by Windows' own `MySQL80` service or by a container nobody
//! remembers, none of which has a `services` row — so the question is asked by *binding* the port,
//! and a preferred port lost to a program MixEngine does not manage is reported with as much of
//! that program's identity as the OS will give up (T38) rather than renumbered in silence. The
//! table is consulted as well and for the opposite reason: a stopped MariaDB holds 3306 as surely
//! as a running one, because the number is in its rendered configuration and in somebody's `.env`.
//!
//! **The search is bounded.** Running out of it is an error, not a longer loop — see
//! [`Error::PortsExhausted`](crate::Error::PortsExhausted).
//!
//! **Allocating and inserting are one critical section**, or two `service.create` calls arriving
//! together are each handed the same next-free port and the second server fails to bind at start.
//! That lock is `hold`, and [`create`](super::create) takes it for every create rather than for
//! the allocating ones alone.
//!
//! What is deliberately *not* here is moving a port afterwards: an allocated port belongs to its row
//! for as long as the row lives, because it is in a project's `.env` and in a colleague's shell
//! history by the end of the afternoon. Deleting whoever holds 3306 promotes nobody into it. Moving
//! one is a person's decision and a regeneration — `mix service set`, which does not exist yet.
//!
//! [`Recipe::preferred_port`]: crate::generate::Recipe::preferred_port

use std::net::IpAddr;

use mixengine_platform::Host;
use mixengine_proto::PortMoved;

use crate::{Result, Store};

/// The port a service was given, and what it would have preferred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Allocation {
    /// The number that goes into the row.
    pub port: u16,

    /// [`None`] when the preferred port was free, and the story otherwise.
    pub moved_from: Option<PortMoved>,
}

/// Held from before a port is chosen until the row that holds it exists.
///
/// **Allocating and inserting are one critical section, or the two `service.create` calls a GUI
/// makes when somebody clicks twice are handed the same next-free port** — both read a table
/// neither has written to yet, both bind-test a port neither has taken, and the second server fails
/// at start with a number that was free when it was chosen. Taken for *every* create rather than
/// for the allocating ones alone: a fixed port inserted between another call's read and its insert
/// is the same collision arriving by the other door.
static IN_FLIGHT: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Take the allocation lock, and hold it until the row is written.
///
/// `pub(crate)` since T81: an extension install allocates several ports and writes two tables, and
/// it has to hold the same lock a `service.create` does or the two can be handed one number.
pub(crate) async fn hold() -> tokio::sync::MutexGuard<'static, ()> {
    IN_FLIGHT.lock().await
}

/// The address a service's `bind_addr` column means.
///
/// **An address this cannot read is loopback**, which is the column's own default and the only
/// answer that is safe to guess: the alternative is refusing to create a service over a spelling
/// the row is allowed to hold, and a probe on the wrong address is a probe that can only be too
/// cautious.
pub(crate) fn bind_address(column: Option<&str>) -> IpAddr {
    column
        .and_then(|address| address.parse().ok())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

/// The lowest free port at or above `preferred`.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the table cannot be read.
pub async fn allocate(
    store: &Store,
    host: &dyn Host,
    bind: IpAddr,
    preferred: u16,
) -> Result<Allocation> {
    // Read once rather than per candidate: the whole search is one pass over a handful of rows, and
    // a query inside the loop would be a round trip per port on the machine where this matters most.
    let taken = held(store).await?;

    let free = |port: u16| !taken.contains(&i64::from(port)) && bindable(bind, port);

    if free(preferred) {
        return Ok(Allocation {
            port: preferred,
            moved_from: None,
        });
    }

    let moved_from = Some(moved(host, preferred));
    let last = preferred.saturating_add(SEARCH);

    for port in preferred.saturating_add(1)..=last {
        if free(port) {
            return Ok(Allocation { port, moved_from });
        }
    }

    Err(crate::Error::PortsExhausted { preferred, last })
}

/// The lowest free port strictly above `after`, for the activator of the service listening there.
///
/// **Near the service it belongs to, deliberately.** An activator's port is in a rendered site file
/// and nowhere else, so it needs no memorable number — but a person reading a listening table wants
/// to see 9001 beside 9000 rather than a stranger from the other end of the range.
///
/// **Every port any row holds is taken, whichever column holds it.** [`allocate`] reads `port`
/// alone, which is right for a service's own address; an activator that did the same would hand two
/// services one address, and the failure would be a bind refused with no explanation attached to it.
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the table cannot be read, and
/// [`Error::PortsExhausted`](crate::Error::PortsExhausted) when the search runs out — bounded for
/// [`allocate`]'s reason: running out is an error rather than a longer loop.
pub async fn allocate_activation(
    store: &Store,
    host: &dyn Host,
    bind: IpAddr,
    after: u16,
) -> Result<u16> {
    let _ = host;

    let taken = held(store).await?;

    let first = after.saturating_add(1);
    let last = first.saturating_add(SEARCH);

    for port in first..=last {
        if !taken.contains(&i64::from(port)) && bindable(bind, port) {
            return Ok(port);
        }
    }

    Err(crate::Error::PortsExhausted {
        preferred: first,
        last,
    })
}

/// Every port any row in this database holds, whichever column holds it.
///
/// **One query rather than one per caller** — roadmap task **T81**. Both allocators ask the same
/// question and a service handed another's address fails as a refused bind with no explanation
/// attached to it, so two lists that could drift apart are two chances to hand one out twice. The
/// third source is `extension_ports`: an extension asks for more ports than a `services` row has
/// columns, and the ones that do not fit are no less held for living in another table (the T81
/// design's D8).
///
/// # Errors
///
/// [`Error::Database`](crate::Error::Database) when the tables cannot be read.
async fn held(store: &Store) -> Result<Vec<i64>> {
    sqlx::query_scalar!(
        r#"SELECT port AS "port!: i64" FROM services WHERE port IS NOT NULL
           UNION
           SELECT activation_port FROM services WHERE activation_port IS NOT NULL
           UNION
           SELECT port FROM extension_ports
           ORDER BY port"#
    )
    .fetch_all(store.pool())
    .await
    .map_err(|source| store.failure("read", source))
}

/// How far above a recipe's preferred port the search goes before it gives up.
const SEARCH: u16 = 64;

/// Whether this machine will let a server have `port` on `bind`, right now.
///
/// **A bind and not a question about a table of listeners.** What the caller is about to write down
/// is a number a server will bind, and the only thing that answers "may it" is the same call the
/// server itself will make — a port can be unavailable for reasons no listener explains, and on
/// Windows an exclusive reservation is one of them.
fn bindable(bind: IpAddr, port: u16) -> bool {
    std::net::TcpListener::bind((bind, port)).is_ok()
}

/// Who holds `preferred`, as much of it as this account may learn.
///
/// **A failure to ask is an empty answer, never a failure of the allocation.** The port is taken
/// either way, and the sentence the caller ends up with — "another program on this machine has it"
/// — is worse than naming the program and better than refusing to create the service at all. See
/// [`mixengine_platform::PortOwner`].
fn moved(host: &dyn Host, preferred: u16) -> PortMoved {
    let holder = host.port_owner().listening_on(preferred).ok().flatten();

    PortMoved {
        preferred,
        pid: holder.as_ref().and_then(|holder| holder.pid),
        program: holder.and_then(|holder| holder.name),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::atomic::{AtomicU16, Ordering};

    use mixengine_platform::{PortHolder, mock};

    use super::*;

    /// The home the mock host is given. Nothing in this module touches it.
    const HOME: &str = "/mixengine";

    /// The address every service in these tests binds.
    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    async fn store() -> (tempfile::TempDir, Store) {
        let home = tempfile::tempdir().expect("a temporary directory");
        let store = Store::open(&home.path().join(crate::paths::DATABASE_FILE_NAME))
            .await
            .expect("a database");
        (home, store)
    }

    /// The lowest port these tests will consider.
    ///
    /// **Below every ephemeral floor** — 32768 on Linux, 49152 on Windows and macOS — and that is
    /// the whole point of the number. See [`a_free_port`].
    const FIRST_PORT: u16 = 24_000;

    /// The highest port these tests will consider, kept clear of the lowest floor of the three.
    const LAST_PORT: u16 = 32_000;

    /// How much of the band each call to [`a_free_port`] gets to itself.
    ///
    /// Wider than [`SEARCH`] on purpose: the test that exhausts the search writes out every port
    /// from its own up to `preferred + SEARCH`, and a window narrower than that would have it
    /// reasoning about a number the next test was given.
    const WINDOW: u16 = 128;

    /// How far into the band the next call starts looking, so two tests running at once cannot be
    /// handed the same number and then disagree about who holds it.
    static NEXT_WINDOW: AtomicU16 = AtomicU16::new(0);

    /// A port this machine is free to give, out of a range nobody is handed by accident.
    ///
    /// **Not the answer to `bind(0)`, which is what this used to be.** That answer comes out of the
    /// dynamic range — 49152 and up on Windows — and the listener has to be dropped before
    /// [`allocate`] can bind the number itself, so between the two moments it belongs to nobody. The
    /// gap is not an instant either: `allocate` reads the `services` table before it probes.
    ///
    /// That range is precisely where the OS serves every `bind(0)` and every outgoing connection on
    /// the machine, and where a closed connection keeps its number unbindable in `TIME_WAIT` for
    /// minutes afterwards. On the Windows leg of CI the whole workspace suite runs there at once,
    /// against real servers. It asked for 57944 and was given 57945: something had taken 57944 in
    /// the gap, and the allocator stepped over it exactly as it is supposed to. **What held it was
    /// not captured, and this does not depend on knowing** — under the dynamic floor the OS hands
    /// the number to nobody, so only a program binding that exact port could take it, and nothing in
    /// this workspace binds a fixed one.
    ///
    /// Still answered by *binding* rather than written down, because a constant this file merely
    /// hoped for is a number some other program on the machine is entitled to hold. What changed is
    /// where it looks.
    fn a_free_port() -> u16 {
        a_free_run(1)
    }

    /// The same, for a test whose claim is about `run` *consecutive* numbers.
    ///
    /// The rule this module states is "the lowest free port at or above the preferred one", and a
    /// test that means to hold it to the letter has to know that the next number up is free as well
    /// — otherwise the allocator stepping over a third program is indistinguishable from the
    /// allocator getting the rule wrong.
    fn a_free_run(run: u16) -> u16 {
        let base = FIRST_PORT + NEXT_WINDOW.fetch_add(WINDOW, Ordering::Relaxed);

        // The property this whole function exists for, asserted rather than hoped for: enough calls
        // would walk the band up into the dynamic range and quietly put the flake back.
        assert!(
            base + WINDOW <= LAST_PORT,
            "these tests have walked out of the band they are safe in; move FIRST_PORT down"
        );

        (base..=base + WINDOW - run)
            .find(|start| {
                (*start..start + run)
                    .all(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok())
            })
            .expect("a machine running these tests has a free run in the window it was given")
    }

    /// A `services` row holding `port`, as a `service.create` before this one would have left it.
    async fn declared(store: &Store, id: &str, port: u16) {
        sqlx::query(
            "INSERT INTO packages (name, version, install_path, installed_at, source_url, sha256)
             VALUES (?, '1.0.0', '/packages/whatever', '2026-08-21T00:00:00Z',
                     'https://example.invalid/whatever', 'abc')",
        )
        .bind(id)
        .execute(store.pool())
        .await
        .expect("a package");

        sqlx::query(
            "INSERT INTO services (id, package_id, instance_name, state, port)
             SELECT ?, id, 'main', 'stopped', ? FROM packages WHERE name = ?",
        )
        .bind(format!("{id}@main"))
        .bind(i64::from(port))
        .bind(id)
        .execute(store.pool())
        .await
        .expect("a service");
    }

    /// An installed extension holding `port`, as an install would have left it.
    async fn held_by_an_extension(store: &Store, id: &str, name: &str, port: u16) {
        sqlx::query(
            "INSERT OR IGNORE INTO extensions
               (id, name, version, kind, manifest_json, install_dir, data_dir, source, signed,
                installed_at)
             VALUES (?, ?, '1.0.0', 'service', '{}', '/x/extensions/x', '/x/data/extensions/x',
                     'registry', 1, '2026-09-02T09:00:00Z')",
        )
        .bind(id)
        .bind(id)
        .execute(store.pool())
        .await
        .expect("an extension");

        sqlx::query("INSERT INTO extension_ports (extension_id, name, port) VALUES (?, ?, ?)")
            .bind(id)
            .bind(name)
            .bind(i64::from(port))
            .execute(store.pool())
            .await
            .expect("a held port");
    }

    /// **A port an extension holds is a port nothing else is handed** — roadmap task **T81**, its
    /// design's D8.
    ///
    /// An extension asks for more ports than a `services` row has columns — Mailpit wants one for
    /// its UI and one for SMTP — so the rest live in `extension_ports`. If this query did not read
    /// that table, a database created next week would be handed the port Mailpit answers SMTP on,
    /// and the failure would arrive as a refused bind with nothing attached to it explaining why.
    #[tokio::test]
    async fn a_port_an_extension_holds_is_not_offered_again() {
        let (_home, store) = store().await;
        let wanted = a_free_run(2);

        held_by_an_extension(&store, "mailpit", "smtp_port", wanted).await;

        let allocation = allocate(&store, &mock::Host::with_home(HOME), LOOPBACK, wanted)
            .await
            .expect("an allocation");

        assert_ne!(allocation.port, wanted, "the extension's own port");
        assert_eq!(allocation.port, wanted + 1);
        assert!(
            allocation.moved_from.is_some(),
            "a service moved off its preferred port and was not told why"
        );
    }

    /// And an activator is handed one no more than a service is.
    #[tokio::test]
    async fn an_activation_port_is_never_a_port_an_extension_holds() {
        let (_home, store) = store().await;
        let first = a_free_run(3);

        declared(&store, "one", first).await;
        held_by_an_extension(&store, "mailpit", "ui_port", first + 1).await;

        let port = allocate_activation(&store, &mock::Host::with_home(HOME), LOOPBACK, first)
            .await
            .expect("an activation port");

        assert_eq!(port, first + 2, "the extension's port was handed out again");
    }

    /// **An activator's port is never a port some row already holds** — roadmap task **T70**, D3.
    ///
    /// The trap this closes is `port + 1`. With pools on 9000 and 9001 that rule gives the first
    /// pool's activator the second pool's own port: one of the two fails to bind, and what the user
    /// is told is a conflict about a number nobody chose.
    #[tokio::test]
    async fn an_activation_port_is_never_a_port_another_row_holds() {
        let (_home, store) = store().await;
        let first = a_free_run(2);

        declared(&store, "one", first).await;
        declared(&store, "two", first + 1).await;

        let port = allocate_activation(&store, &mock::Host::with_home(HOME), LOOPBACK, first)
            .await
            .expect("an activation port");

        assert_ne!(port, first, "the pool's own port");
        assert_ne!(port, first + 1, "the other pool's own port");
        assert!(port > first, "{port} is not above the pool it belongs to");
    }

    /// **The other column is as taken as the first one.**
    ///
    /// `allocate` reads `port` alone, which is right for a service's own address. An activator that
    /// did the same would hand two services one address, and neither of them would say why.
    #[tokio::test]
    async fn an_activation_port_is_never_another_rows_activator() {
        let (_home, store) = store().await;
        let first = a_free_run(2);

        declared(&store, "one", first).await;
        sqlx::query("UPDATE services SET activation_port = ? WHERE id = 'one@main'")
            .bind(i64::from(first + 1))
            .execute(store.pool())
            .await
            .expect("an activator");

        let port = allocate_activation(&store, &mock::Host::with_home(HOME), LOOPBACK, first)
            .await
            .expect("an activation port");

        assert_ne!(
            port,
            first + 1,
            "another row's activator holds its number as surely as a service holds its port"
        );
    }

    /// The ordinary case: the recipe's own number, and nothing to tell the user about.
    #[tokio::test]
    async fn the_preferred_port_is_what_a_service_gets_when_nobody_holds_it() {
        let (_home, store) = store().await;
        let preferred = a_free_port();

        let allocation = allocate(&store, &mock::Host::with_home(HOME), LOOPBACK, preferred)
            .await
            .expect("an allocation");

        assert_eq!(allocation.port, preferred);
        assert_eq!(
            allocation.moved_from, None,
            "a service that got what it asked for has nothing to explain"
        );
    }

    /// The port is tested by *binding* it, so a program with no `services` row still holds it.
    ///
    /// 3306 on a developer's machine is routinely an XAMPP or Windows' own `MySQL80`, and a
    /// question asked of the table would have answered that it was free. The step over is only half
    /// of it: who took it is read from the OS and carried back, because a service that silently
    /// moved is one whose `.env` is wrong for a reason nobody was given.
    #[tokio::test]
    async fn a_port_another_program_is_listening_on_is_stepped_over_and_the_program_named() {
        let (_home, store) = store().await;

        // Out of the band and held for the length of the test, rather than whatever `bind(0)` says:
        // this test needs a port that is genuinely occupied, and the one rule this module's tests
        // keep is that none of their numbers comes out of the range the OS hands round.
        let held = a_free_port();
        let _squatter = TcpListener::bind((Ipv4Addr::LOCALHOST, held)).expect("a squatter");

        let host = mock::Host::with_a_port_held(
            HOME,
            held,
            PortHolder {
                pid: Some(4242),
                name: Some("mysqld.exe".to_owned()),
            },
        );

        let allocation = allocate(&store, &host, LOOPBACK, held)
            .await
            .expect("an allocation");

        assert!(
            allocation.port > held,
            "a port somebody is listening on was handed out anyway"
        );
        assert_eq!(
            allocation.moved_from,
            Some(PortMoved {
                preferred: held,
                pid: Some(4242),
                program: Some("mysqld.exe".to_owned()),
            })
        );
    }

    /// The step over is exactly one port, not merely some port above.
    ///
    /// **This claim can only be made where the band is somebody's**, which is why it is here rather
    /// than in an end-to-end suite. `crates/mixengine-cli/tests/service.rs` asks the same question of
    /// a real daemon and deliberately asks it more weakly: on a machine where anything may take a
    /// number between two binds, "consecutive" is a statement about the machine and not about this
    /// module — and it went red on `test (windows-latest)` for exactly that reason. Here the two
    /// numbers were free a moment ago and nothing on the machine is handed one by accident, so the
    /// letter of the rule is what is asserted.
    #[tokio::test]
    async fn a_moved_service_is_given_the_very_next_port() {
        let (_home, store) = store().await;
        let held = a_free_run(2);
        let _squatter = TcpListener::bind((Ipv4Addr::LOCALHOST, held)).expect("a squatter");

        let allocation = allocate(&store, &mock::Host::with_home(HOME), LOOPBACK, held)
            .await
            .expect("an allocation");

        assert_eq!(
            allocation.port,
            held + 1,
            "the free port immediately above {held} was passed over"
        );
    }

    /// A number another row already holds is taken, whatever the machine says about it right now.
    ///
    /// This is the half a bind cannot answer: a MariaDB that is *stopped* holds 3306 as surely as
    /// one that is running, because the number is in its rendered configuration and in somebody's
    /// `.env`, and handing it to a MySQL created this afternoon would mean two servers that can
    /// never run at once. Nothing is named in the answer, for the same reason: the service holding
    /// it may have no process at all.
    #[tokio::test]
    async fn a_port_another_service_was_already_given_is_not_offered_a_second_time() {
        let (_home, store) = store().await;
        let held = a_free_port();
        declared(&store, "mariadb", held).await;

        let allocation = allocate(&store, &mock::Host::with_home(HOME), LOOPBACK, held)
            .await
            .expect("an allocation");

        assert!(
            allocation.port > held,
            "a port a stopped service holds was handed out again"
        );
        assert_eq!(
            allocation.moved_from,
            Some(PortMoved {
                preferred: held,
                pid: None,
                program: None,
            })
        );
    }

    /// Running out of the search is reported, and is not answered by searching further.
    ///
    /// A home with sixty-five consecutive services above 3306 is not one more probe away from
    /// working, and a database that quietly landed three hundred ports from the number its product
    /// is documented under would hide whatever is actually wrong here.
    #[tokio::test]
    async fn running_out_of_the_search_is_an_error_and_not_a_longer_loop() {
        let (_home, store) = store().await;
        let preferred = a_free_port();
        let last = preferred.saturating_add(SEARCH);

        for (nth, port) in (preferred..=last).enumerate() {
            declared(&store, &format!("occupier-{nth}"), port).await;
        }

        let refused = allocate(&store, &mock::Host::with_home(HOME), LOOPBACK, preferred)
            .await
            .expect_err("nothing was free");

        assert!(
            matches!(
                refused,
                crate::Error::PortsExhausted { preferred: asked, last: reached }
                    if asked == preferred && reached == last
            ),
            "{refused:?}"
        );
    }
}
