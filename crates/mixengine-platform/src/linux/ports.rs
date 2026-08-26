//! Who is listening on a local TCP port, on Linux.
//!
//! Two steps, because the kernel publishes them in two places. `/proc/net/tcp` and its `tcp6`
//! sibling say *that* a port is being listened on and which socket inode it is; nothing there says
//! which process holds that inode. The second half is a walk of `/proc/<pid>/fd`, looking for the
//! symlink that reads `socket:[<inode>]`.
//!
//! **The second half is the one this account may be refused.** `/proc/<pid>/fd` is readable by the
//! process's own user and by root, so a listener belonging to another account — a distribution's
//! `mysqld` running as `mysql`, a container's published port — is visible in the table and traceable
//! to nobody. That is not a failure and must not be reported as "nothing is listening": it is
//! [`PortHolder`] with no pid, which is the whole reason both of its fields are optional. `ss -ltnp`
//! and `lsof` are refused in exactly the same way, so there is nothing to gain by shelling out to
//! either.

use std::fs;
use std::path::Path;

use crate::{PortHolder, PortOwner, Result};

/// The two tables, in the order they are asked.
///
/// A socket bound to `::` is in the second alone and covers IPv4 through it, so a lookup that read
/// only the first would miss every dual-stack server.
const TABLES: [&str; 2] = ["/proc/net/tcp", "/proc/net/tcp6"];

/// `TCP_LISTEN`, as `/proc/net/tcp` spells a socket state.
const LISTEN: &str = "0A";

/// `TCP_ESTABLISHED`, as the same column spells it.
///
/// Its neighbour [`LISTEN`] is what a *start* collides with; this is what a *use* looks like. Two
/// constants over one table because the two questions are opposites — a listening row says the port
/// is taken, an established row says somebody is on the other end of it — and a running service
/// shows both at once.
const ESTABLISHED: &str = "01";

#[derive(Debug)]
pub(crate) struct Ports;

impl PortOwner for Ports {
    fn listening_on(&self, port: u16) -> Result<Option<PortHolder>> {
        for table in TABLES {
            // A missing table is a kernel built without that family, not a failure to diagnose:
            // `/proc/net/tcp6` is absent on a machine booted with `ipv6.disable=1`.
            let Ok(text) = fs::read_to_string(table) else {
                continue;
            };

            if let Some(holder) = holder(&text, port) {
                return Ok(Some(holder));
            }
        }

        Ok(None)
    }
}

impl crate::ConnectionCount for Ports {
    fn established_on(&self, port: u16) -> Result<usize> {
        let mut total = 0;

        for table in TABLES {
            // Absent for the reason `listening_on` skips it: a kernel booted with `ipv6.disable=1`
            // publishes no `tcp6`. Unlike the listener lookup this one sums rather than stops at the
            // first answer — a dual-stack server's clients are spread across both tables.
            let Ok(text) = fs::read_to_string(table) else {
                continue;
            };

            total += established_in(&text, port);
        }

        Ok(total)
    }
}

/// What one table says about `port`, if anything.
fn holder(table: &str, port: u16) -> Option<PortHolder> {
    let inode = inode_of(table, port)?;
    let pid = process_of(inode);

    Some(PortHolder {
        pid,
        name: pid.and_then(name_of),
    })
}

/// The socket inode listening on `port` in this table, if one is.
///
/// The columns are `sl local_address rem_address st … inode`, so this reads the second, the fourth
/// and the tenth. Anything shorter than that is a header line or a kernel this code does not know,
/// and is skipped rather than guessed at.
fn inode_of(table: &str, port: u16) -> Option<u64> {
    table.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let local = fields.nth(1)?;
        let state = fields.nth(1)?;
        let inode = fields.nth(5)?;

        (state == LISTEN && local_port(local) == Some(port))
            .then(|| inode.parse().ok())
            .flatten()
    })
}

/// How many rows of this table are connections established to `port`.
///
/// Reads two of the columns [`inode_of`] reads and none of the rest: who holds the socket is the
/// question the listener path asks, and walking `/proc` for it here would cost a directory scan per
/// connection per sweep to produce a pid nothing reads.
///
/// A free function over the text, like [`inode_of`], so the parsing is testable against a captured
/// table rather than against whatever this machine happens to have open.
fn established_in(table: &str, port: u16) -> usize {
    table
        .lines()
        .filter(|line| {
            let mut fields = line.split_whitespace();

            let Some(local) = fields.nth(1) else {
                return false;
            };

            let Some(state) = fields.nth(1) else {
                return false;
            };

            state == ESTABLISHED && local_port(local) == Some(port)
        })
        .count()
}

/// The port out of a `local_address` column, which is `<address>:<port>` in hexadecimal.
///
/// The address half is four hex digits per IPv4 byte or thirty-two for IPv6 and is deliberately not
/// read: a listener on `127.0.0.1` and one on `0.0.0.0` collide with the same start, and the whole
/// question here is whether the port is taken.
fn local_port(local: &str) -> Option<u16> {
    u16::from_str_radix(local.rsplit_once(':')?.1, 16).ok()
}

/// The process holding socket `inode`, where this account may see it.
///
/// `None` for the refusal this module's own documentation is mostly about, and for the race where
/// the socket is closed between reading the table and walking `/proc`.
fn process_of(inode: u64) -> Option<u32> {
    let socket = format!("socket:[{inode}]");
    let socket = Path::new(&socket);

    let processes = fs::read_dir("/proc").ok()?;

    processes.flatten().find_map(|process| {
        let pid: u32 = process.file_name().to_str()?.parse().ok()?;

        // Unreadable is the common case rather than an error: every process belonging to another
        // account is one, and `/proc` also holds entries that end between the two reads.
        let descriptors = fs::read_dir(process.path().join("fd")).ok()?;

        descriptors
            .flatten()
            .any(|descriptor| fs::read_link(descriptor.path()).is_ok_and(|target| target == socket))
            .then_some(pid)
    })
}

/// The file name of the program `pid` is running.
///
/// `/proc/<pid>/exe` first because it is the whole path and is not truncated; `comm` is the fallback
/// for the case where the link may not be read, and it is capped at fifteen characters by the
/// kernel — which is why it is not the first choice for a name a user has to recognise.
fn name_of(pid: u32) -> Option<String> {
    let process = Path::new("/proc").join(pid.to_string());

    if let Ok(program) = fs::read_link(process.join("exe"))
        && let Some(name) = program.file_name()
    {
        return Some(name.to_string_lossy().into_owned());
    }

    fs::read_to_string(process.join("comm"))
        .ok()
        .map(|comm| comm.trim().to_owned())
        .filter(|comm| !comm.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two rows out of a real `/proc/net/tcp`, with the columns after the inode trimmed.
    ///
    /// `0CEA` is 3306 and the row is `0A`, listening. `1F90` is 8080 and the row is `01`,
    /// established — a connection *to* something on 8080, which is the case a port check must not
    /// confuse with a server holding it.
    const TABLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 424242 1
   1: 0100007F:1F90 0100007F:BC1E 01 00000000:00000000 00:00000000 00000000  1000        0 424243 1
";

    /// One listener and two connections on 3306, and one connection on 8080.
    ///
    /// The shape [`TABLE`] deliberately does not have: a port that is *both* listened on and in use,
    /// which is what every running service looks like and is the only arrangement that can tell the
    /// two readers here apart.
    const BUSY: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0CEA 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 424242 1
   1: 0100007F:0CEA 0100007F:B3A2 01 00000000:00000000 00:00000000 00000000  1000        0 424243 1
   2: 0100007F:0CEA 0100007F:B3A3 01 00000000:00000000 00:00000000 00000000  1000        0 424244 1
   3: 0100007F:1F90 0100007F:B3A4 01 00000000:00000000 00:00000000 00000000  1000        0 424245 1
";

    #[test]
    fn a_listening_row_is_found_by_its_port() {
        assert_eq!(inode_of(TABLE, 3306), Some(424_242));
    }

    /// Established rows are counted and the listener among them is not.
    ///
    /// The two states are neighbours in one column of one file — `0A` is a listener and `01` is a
    /// live connection — and reading the wrong one would report every busy service as idle. This is
    /// the assertion the idle sweeper rests on.
    #[test]
    fn established_rows_are_counted_and_listeners_are_not() {
        assert_eq!(
            established_in(BUSY, 3306),
            2,
            "two connections, and the listening row is not one of them"
        );
        assert_eq!(established_in(BUSY, 8080), 1);
    }

    /// A port nothing is connected to counts zero, whether or not it is listening.
    ///
    /// Both halves matter to the caller: an idle service still holds its port, so "no connections"
    /// has to be a real answer about a port that is very much in the table.
    #[test]
    fn a_port_with_no_connections_counts_none() {
        assert_eq!(
            established_in(TABLE, 3306),
            0,
            "3306 is listening in this table and nobody is connected to it"
        );
        assert_eq!(established_in(TABLE, 5432), 0);
    }

    #[test]
    fn a_port_something_is_merely_connected_to_is_not_listening() {
        assert_eq!(
            inode_of(TABLE, 8080),
            None,
            "8080 appears in the table as an established connection, not as a listener"
        );
    }

    #[test]
    fn a_port_the_table_does_not_mention_has_no_inode() {
        assert_eq!(inode_of(TABLE, 5432), None);
    }

    /// The refusal this module exists to render honestly, staged the only way a test can stage it.
    ///
    /// A listener belonging to another account cannot be created here, and its inode would be
    /// traceable to nobody for exactly the reason this fabricated one is: `/proc` holds no process
    /// this account may read that owns it. What must not happen is the answer collapsing to "nothing
    /// is listening on 3306", which is the sentence that would send a user looking at their own
    /// configuration.
    #[test]
    fn a_listening_socket_this_account_cannot_trace_is_still_a_holder() {
        let holder = holder(TABLE, 3306).expect("the table says 3306 is being listened on");

        assert_eq!(
            holder.pid, None,
            "no process here owns the fabricated inode"
        );
        assert_eq!(holder.name, None, "and a pid nobody has cannot be named");
    }
}
