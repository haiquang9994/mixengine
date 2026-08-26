//! Who is listening on a local TCP port, on macOS.
//!
//! The one implementation of this capability that runs a program rather than reading a table, and
//! the reason is what the alternative would be. macOS publishes no `/proc`, and the only interface
//! that maps a socket to a process is `libproc` — `proc_listpids`, then `proc_pidfdinfo` per
//! descriptor, filling a `socket_fdinfo` whose layout the `libc` crate does not declare. Getting
//! that layout wrong does not fail to compile; it reads whatever is at those offsets and reports it
//! as a pid. `lsof` ships with every macOS, is the tool Apple's own documentation points at, and is
//! asked here in the two most boring ways it can be asked.
//!
//! **Two one-token answers rather than one parsed table.** `-t` prints a pid per line and nothing
//! else, and `ps -o comm=` prints one path; neither has columns, headers or a locale. `lsof -F` is
//! the machine-readable format built for this, and it would still be a format to get wrong.

use std::path::Path;
use std::process::Command;

use crate::{Error, PortHolder, PortOwner, Result};

/// Absolute, because the daemon's `PATH` is its own and a diagnosis must not depend on it.
///
/// Both are part of the base system — `lsof` has shipped in `/usr/sbin` since Mac OS X 10.0 and
/// `ps` in `/bin` since before that — so neither is a dependency a user could be missing.
const LSOF: &str = "/usr/sbin/lsof";

/// As [`LSOF`].
const PS: &str = "/bin/ps";

#[derive(Debug)]
pub(crate) struct Ports;

impl PortOwner for Ports {
    fn listening_on(&self, port: u16) -> Result<Option<PortHolder>> {
        let Some(pid) = pid_on(port) else {
            return Ok(None);
        };

        Ok(Some(PortHolder {
            pid: Some(pid),
            name: name_of(pid),
        }))
    }
}

impl crate::ConnectionCount for Ports {
    fn established_on(&self, port: u16) -> Result<usize> {
        // Same two boring questions as `pid_on` below, with the state filter changed: `-t` prints
        // one pid per line and one line per matching socket, so the count of lines is the count of
        // connections. Two clients from one process are two lines, which is what makes the terse
        // form correct here rather than a listing that would need de-duplicating.
        let connected = Command::new(LSOF)
            .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:ESTABLISHED", "-t"])
            .output()
            .map_err(|source| Error::Io {
                action: "count the connections to a port",
                path: std::path::PathBuf::from(LSOF),
                source,
            })?;

        // **Unlike `pid_on`, this one distinguishes.** There, every way of finding nobody is the
        // same answer on a path that is already failing; here, "nothing is connected" stops a
        // service and "I could not ask" must not — so an exit that is neither success nor `lsof`'s
        // documented "matched nothing" is raised rather than counted as zero.
        match connected.status.code() {
            Some(0) => Ok(String::from_utf8_lossy(&connected.stdout)
                .split_whitespace()
                .count()),
            Some(1) => Ok(0),
            _ => Err(Error::Command {
                command: "lsof",
                path: None,
                status: connected.status.to_string(),
                output: String::from_utf8_lossy(&connected.stderr).trim().to_owned(),
            }),
        }
    }
}

/// The pid listening on `port`, if this account can see one.
///
/// **Nothing here is an error, and that is deliberate.** `lsof` exits non-zero when it matched
/// nothing at all, which is the ordinary answer to this question, and it exits non-zero again when
/// it was refused — and both arrive as an empty list of pids. A caller of this capability is on an
/// error path already (see [`PortOwner`]), so the distinction it could act on is "somebody is
/// listening" against "nobody is", never which of the two ways nobody was found.
fn pid_on(port: u16) -> Option<u32> {
    let listeners = Command::new(LSOF)
        .args([
            // No name resolution and no port-name lookup: this asks about a number and must be
            // answered with numbers, whatever `/etc/services` calls 3306 on this machine.
            "-nP",
            &format!("-iTCP:{port}"),
            "-sTCP:LISTEN",
            // Terse: one pid per line, no header, no columns.
            "-t",
        ])
        .output()
        .ok()?;

    String::from_utf8_lossy(&listeners.stdout)
        .split_whitespace()
        .find_map(|pid| pid.parse().ok())
}

/// The file name of the program `pid` is running.
///
/// `comm=` is the executable's path with the header suppressed, so the whole answer is one line.
/// `None` where `ps` cannot see the process, which on macOS is rarer than the Linux and Windows
/// refusals this mirrors — a process belonging to another user is listed — but is still the case
/// when it ends between the two commands.
fn name_of(pid: u32) -> Option<String> {
    let described = Command::new(PS)
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;

    let program = String::from_utf8_lossy(&described.stdout).trim().to_owned();

    Path::new(&program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}
