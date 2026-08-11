//! Starting a process that outlives the one that started it.
//!
//! This is what `mixengined --detach` and a client's autostart (roadmap task T10) are made of, and
//! all of it is the part every OS does differently. Surviving the parent is the easy half and comes
//! for free on both systems; what has to be arranged is that the child stops being *attached* to
//! whoever started it:
//!
//! - it must not be reached by a Ctrl-C meant for the terminal the parent was typed into,
//! - it must not write into that terminal hours later, and
//! - it must not keep the terminal's console alive on Windows.
//!
//! The stdio redirection is the one piece that is genuinely shared, so it is done here: all three
//! streams go to the null device. A daemon that has detached says everything it has to say in
//! `logs/daemon.log`, and inheriting the parent's handles is how a background process ends up
//! printing into a shell prompt somebody is using.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::sys::process as sys;
use crate::{Error, Result};

/// A process that has been started and let go.
///
/// Dropping it does **not** stop the child — neither system kills a process when its parent lets go
/// of the handle, which is the entire point. What the handle is still good for is the question the
/// caller asks while it waits for the child to come up: is it still there?
///
/// **A caller that goes on running has to reap it.** [`Child`] does not wait on drop, so on Unix the
/// process started here stays this process's child and becomes a zombie the moment it exits, for as
/// long as this process lives. `mixengined --detach` exits straight after the spawn and never meets
/// that; `mix` and the GUI autostarting a daemon (roadmap task T10) will, and have to keep the
/// handle and go on asking [`exited`](Self::exited) rather than dropping it once the daemon answers.
#[derive(Debug)]
pub struct Detached {
    child: Child,
}

/// How a detached process ended.
///
/// Two questions in one answer, because the caller needs both and they come from the same syscall.
/// [`is_success`](Self::is_success) is what a decision is made on — a `--detach` whose child exited
/// *successfully* because another daemon already held the home has not failed and must keep waiting
/// for that daemon, while one whose child failed has nothing left to wait for. The [`Display`](fmt::Display)
/// form is for the message to a person, and is rendered rather than reduced to a code because the
/// two systems disagree about what one is: a Unix child that died on a signal has no exit code at
/// all, and reporting `None` for it would lose the only interesting thing about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    success: bool,
    described: String,
}

impl Detached {
    /// The child's process id, for a message to a person and for the lock file to be checked
    /// against.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether it has already exited, and how it put it.
    ///
    /// Never waits. A caller polling for a daemon to come up needs this to tell "not ready yet"
    /// from "gone", which are otherwise the same silence — and the second one is the answer that
    /// must not be waited out.
    ///
    /// # Errors
    ///
    /// [`Error::Os`] if the OS will not say. Not the same as "still running",
    /// which is `Ok(None)`.
    pub fn exited(&mut self) -> Result<Option<Exit>> {
        self.child
            .try_wait()
            .map(|status| {
                status.map(|status| Exit {
                    success: status.success(),
                    described: status.to_string(),
                })
            })
            .map_err(|source| Error::Os {
                action: "check on the process it started",
                source,
            })
    }
}

impl Exit {
    /// Whether it ended the way a process that did its job ends.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.success
    }
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.described)
    }
}

/// Start `program` detached from this process and from its terminal, with `directory` as its
/// working directory.
///
/// The environment is inherited, which is deliberate: `MIXENGINE_LOG_FORMAT` is set by a log
/// collector that wraps a command it did not write, and a child that dropped it would stop being
/// collected halfway through a start.
///
/// **`directory` is required rather than inherited**, and the caller is expected to name something
/// it is happy to have held for the child's whole life: a process's working directory is a reference
/// the OS keeps, so a daemon that kept its caller's would stop that directory from being renamed or
/// deleted on Windows and stop its filesystem from being unmounted on Unix. A client autostarting a
/// daemon is typically run from a project folder the person is working in, which is the last
/// directory worth pinning for days. The daemon's own home is the obvious answer.
///
/// # What this does to the calling process on Windows
///
/// It clears `HANDLE_FLAG_INHERIT` on **this process's** standard handles for the length of the
/// spawn, because there is no narrower way to keep the child from inheriting a pipe its caller is
/// reading to end-of-file — `windows/process.rs` has the whole of why. They are put back before this
/// returns, so a caller that goes on to start other children passes its stdio on to them as usual;
/// what remains is that a `CreateProcessW` running *concurrently* on another thread may not inherit
/// them, which is a window `bInheritHandles` already opens for every spawn in the program.
///
/// # Errors
///
/// [`Error::Io`] naming the program when it cannot be started — the usual reasons
/// being a binary that has been moved since this process was launched, or one an antivirus is
/// holding.
pub fn spawn_detached(program: &Path, args: &[OsString], directory: &Path) -> Result<Detached> {
    let mut command = Command::new(program);

    command
        .args(args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Held across the spawn and no further: on Windows detaching a child means turning something
    // off on *this* process, and the drop below is what turns it back on — including when the spawn
    // is the thing that failed.
    let detaching = sys::detach(&mut command);
    let spawned = command.spawn();
    drop(detaching);

    spawned
        .map(|child| Detached { child })
        .map_err(|source| Error::Io {
            action: "start",
            path: program.to_path_buf(),
            source,
        })
}
