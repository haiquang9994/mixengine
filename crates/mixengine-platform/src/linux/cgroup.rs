//! The cgroup v2 a supervised service is capped in, and the subtree it is allowed to live in.
//!
//! Roadmap task **T68**. Three facts shape every line here, and none of them is true on the other
//! two systems:
//!
//! 1. **An unprivileged process may only write the cgroups delegated to it.** systemd's user manager
//!    delegates a subtree to `user@N.service`; a machine with no systemd delegates nothing. Both are
//!    ordinary machines and neither may be assumed, so the boundary is **discovered** —
//!    [`Delegation::discover`] — and never built from a path this code knows the shape of.
//! 2. **Delegation is per controller.** `memory` and `pids` arrive far more readily than `cpu`, and
//!    which of them arrive has moved between systemd releases. So a machine that caps memory and
//!    cannot cap CPU is ordinary too, and [`Delegation::controllers`] answers about each separately.
//! 3. **A process joins a cgroup by writing into it, and the process that writes may be the process
//!    itself.** Writing `0` to `cgroup.procs` means "whoever is writing", which is what lets the
//!    child put itself in between `fork` and `exec` — see [`Cgroup::procs_fd`] — instead of being
//!    put in by the daemon after it is already running and possibly already forking.
//!
//! Everything that can fail here fails into a *sentence*, not an error: a machine that will not lend
//! the mechanism still runs the service, uncapped, and says why. The T68 design, D6.

use std::fs;
use std::io::Write as _;
use std::os::fd::{AsRawFd as _, RawFd};
use std::path::{Path, PathBuf};

use crate::process::Limits;
use crate::{Error, Result};

/// Where cgroup v2 is mounted on every system that has it.
const ROOT: &str = "/sys/fs/cgroup";

/// The directory MixEngine makes its own, inside whatever subtree it was delegated.
///
/// **A plain name, with no `.slice` or `.scope` suffix.** Those suffixes are systemd's vocabulary,
/// and a delegated subtree that uses them invites the manager that delegated it to start managing
/// them back.
const OURS: &str = "mixengine";

/// The subtree this session may write in, and what it will lend of it.
#[derive(Debug, Clone)]
pub(crate) struct Delegation {
    /// `<boundary>/mixengine`, already created. Every service's cgroup is a child of this.
    ours: PathBuf,
}

impl Delegation {
    /// Find the highest cgroup this process may create a directory in.
    ///
    /// **Tested for rather than inferred.** The capability the rest of this needs is "can create a
    /// directory here", so that is the question asked — not who owns the directory, and not whether
    /// the path looks like `user@N.service`. A machine with no systemd has to arrive here and be
    /// told the truth rather than have a path built for it that fails to open.
    ///
    /// # Errors
    ///
    /// A sentence for a person, which becomes an
    /// [`Enforcement::Unavailable`](crate::Enforcement::Unavailable). Never an [`Error`]: not being
    /// able to cap a service is not a failure to do something, it is an answer.
    pub(crate) fn discover() -> std::result::Result<Self, String> {
        let own = own_cgroup()?;

        // Upwards from the daemon's own cgroup. The first ancestor that accepts a `mkdir` is the
        // boundary — going higher would only find directories the kernel refuses us, and stopping
        // lower would give up delegation we were actually granted.
        for ancestor in own.ancestors() {
            if ancestor == Path::new(ROOT) {
                break;
            }

            let ours = ancestor.join(OURS);

            match fs::create_dir(&ours) {
                Ok(()) => return Ok(Self { ours }),

                // Ours from an earlier run of this daemon, or of another one in the same session.
                // Both are fine: what a `Delegation` promises is a directory to make children in.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(Self { ours });
                }

                Err(_) => continue,
            }
        }

        Err(format!(
            "no cgroup in this session may be written to (looked upward from {}) — a delegated \
             subtree is what systemd's user manager provides, and this session has none",
            own.display()
        ))
    }

    /// Whether `cpu` and `memory` can be enabled for the cgroups made under this delegation.
    ///
    /// **Per controller, and that pair is the whole reason
    /// [`Enforcement`](crate::Enforcement) is answered per field.** A subtree may be delegated with
    /// `memory` and without `cpu`, which is what a stock systemd user session often looks like.
    pub(crate) fn controllers(&self) -> Controllers {
        Controllers {
            cpu: self.enable("cpu"),
            memory: self.enable("memory"),
        }
    }

    /// Ask for one controller in this delegation's `cgroup.subtree_control`, and say whether it
    /// arrived.
    ///
    /// Idempotent: `+cpu` on a subtree that already has `cpu` is accepted and changes nothing, so
    /// this is safe to run at every probe rather than only once.
    fn enable(&self, controller: &str) -> std::result::Result<(), String> {
        let path = self.ours.join("cgroup.subtree_control");

        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|mut file| file.write_all(format!("+{controller}").as_bytes()))
            .map_err(|error| {
                format!(
                    "this session does not delegate the {controller} controller ({error}) — a \
                     service's {controller} limit will be stored and not enforced"
                )
            })
    }

    /// Remove the cgroups a killed daemon left behind.
    ///
    /// **An empty cgroup is removable and a non-empty one is not**, which is why this needs no list
    /// of what it expects to find: the kernel refuses to remove a directory that still holds a
    /// process, and a directory that still holds one belongs to a service this daemon is about to
    /// adopt. Every failure is ignored for exactly that reason.
    pub(crate) fn sweep_stale(&self) {
        let Ok(entries) = fs::read_dir(&self.ours) else {
            return;
        };

        for entry in entries.flatten() {
            let _ = fs::remove_dir(entry.path());
        }
    }

    /// Make this service's own cgroup, or answer [`None`] when this machine lends nothing.
    ///
    /// `None` rather than an error, for [`discover`](Self::discover)'s reason: a service whose cap
    /// cannot be applied still starts.
    pub(crate) fn cgroup_for(&self, name: &str) -> Option<Cgroup> {
        Cgroup::create(&self.ours, name).ok()
    }
}

/// Which of the two controllers this delegation will lend, each with its own reason when it will
/// not.
#[derive(Debug, Clone)]
pub(crate) struct Controllers {
    /// `cpu.max`.
    pub(crate) cpu: std::result::Result<(), String>,

    /// `memory.max` and `memory.high`.
    pub(crate) memory: std::result::Result<(), String>,
}

/// One service's cgroup: a directory, and an open handle on the file that admits processes to it.
#[derive(Debug)]
pub(crate) struct Cgroup {
    /// `<boundary>/mixengine/<service>`.
    dir: PathBuf,

    /// `<dir>/cgroup.procs`, opened once in the parent.
    ///
    /// **Held open rather than opened per spawn**, because the moment it is needed is inside a
    /// `pre_exec` closure, where opening a file by path is a great deal more than the
    /// async-signal-safe `write` this reduces to.
    procs: fs::File,
}

impl Cgroup {
    /// Create the directory and open its `cgroup.procs`.
    fn create(parent: &Path, name: &str) -> Result<Self> {
        let dir = parent.join(name);

        match fs::create_dir(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(Error::Io {
                    action: "create a cgroup for a supervised service",
                    path: dir,
                    source,
                });
            }
        }

        let procs = fs::OpenOptions::new()
            .write(true)
            .open(dir.join("cgroup.procs"))
            .map_err(|source| Error::Io {
                action: "open the process list of a supervised service's cgroup",
                path: dir.join("cgroup.procs"),
                source,
            })?;

        Ok(Self { dir, procs })
    }

    /// The descriptor a child writes `0` into to put *itself* in this cgroup.
    ///
    /// See the module header, point 3, and `unix/process.rs`'s `pre_exec` for why the child does it
    /// rather than the daemon.
    pub(crate) fn procs_fd(&self) -> RawFd {
        self.procs.as_raw_fd()
    }

    /// Where this cgroup is, for the test that reads the caps back out of it.
    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Write the ceilings, and leave a field alone when the controller behind it was not lent.
    ///
    /// **`memory.high` is set equal to `memory.max`, not below it.** `memory.high` makes the kernel
    /// reclaim and throttle at the threshold; `memory.max` makes it kill there. Equal means a service
    /// that can be squeezed back under the line is squeezed rather than killed, and one that cannot
    /// is still killed — which is what a development machine wants, and needs no ratio anybody would
    /// have to defend.
    ///
    /// **A failed write is not an error.** A controller this session was not delegated is reported
    /// by [`Controllers`] once, to a person; failing every start over it would turn a machine that
    /// cannot cap a service into one that cannot run one.
    pub(crate) fn write_caps(&self, limits: &Limits) {
        // `$MAX $PERIOD`, per core by construction: `50000 100000` is half of one core whatever the
        // machine has. No conversion, unlike the job object's — see `windows/process.rs::set_cpu`.
        let cpu = limits.cpu_percent.map_or_else(
            || "max 100000".to_owned(),
            |percent| format!("{} 100000", u32::from(percent) * 1000),
        );
        self.write("cpu.max", &cpu);

        let memory = limits.memory_mb.map_or_else(
            || "max".to_owned(),
            |mb| (u64::from(mb) * 1024 * 1024).to_string(),
        );
        self.write("memory.max", &memory);
        self.write("memory.high", &memory);
    }

    /// One cgroup file, written whole, with the failure deliberately dropped.
    fn write(&self, file: &str, value: &str) {
        let _ = fs::write(self.dir.join(file), value);
    }
}

impl Drop for Cgroup {
    /// Take the directory away when the group that owned it goes.
    ///
    /// Failure is ignored and is the ordinary case rather than an exotic one: a cgroup whose
    /// processes have not finished leaving cannot be removed, and
    /// [`Delegation::sweep_stale`] collects it at the next daemon start.
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.dir);
    }
}

/// The cgroup this process is in, from `/proc/self/cgroup`.
///
/// Under cgroup v2 that file holds exactly one line, `0::<path>`. Anything else means this machine
/// is on the legacy hierarchy or on a hybrid one, where none of this applies — and the caller is
/// told so rather than being handed a path that will not behave.
fn own_cgroup() -> std::result::Result<PathBuf, String> {
    let content = fs::read_to_string("/proc/self/cgroup").map_err(|error| {
        format!("this machine does not report a cgroup for its own processes ({error})")
    })?;

    let relative = content
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| {
            "this machine is not running the cgroup v2 unified hierarchy, which is the only one \
             MixEngine can cap a service in"
                .to_owned()
        })?;

    Ok(Path::new(ROOT).join(relative.trim().trim_start_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reading is of this machine, whatever this machine is.
    ///
    /// **Both answers are correct** and which one arrives is a property of how this session was
    /// started — a container, a WSL distribution without systemd, and a desktop login give three
    /// different ones. What is asserted is that the answer is *shaped* right: a path under the
    /// cgroup root, or a sentence a person can act on.
    #[test]
    fn discovery_answers_with_a_subtree_or_with_a_reason() {
        match Delegation::discover() {
            Ok(delegation) => assert!(
                delegation.ours.starts_with(ROOT),
                "a delegated subtree is under the cgroup root: {}",
                delegation.ours.display(),
            ),

            Err(why) => assert!(
                why.len() > 20,
                "the reason is a sentence for a person, not a code: {why}",
            ),
        }
    }

    /// `cpu.max` is written per core, so the number does not depend on the machine.
    #[test]
    fn a_cpu_percentage_is_written_as_a_share_of_one_core() {
        let Ok(delegation) = Delegation::discover() else {
            eprintln!("skipped: this session has no delegated subtree");
            return;
        };

        let Some(cgroup) = delegation.cgroup_for("mixengine-test-cpu") else {
            eprintln!("skipped: this session would not lend a cgroup");
            return;
        };

        cgroup.write_caps(&Limits {
            cpu_percent: Some(50),
            ..Limits::default()
        });

        // Only assert if the controller was actually lent; `write_caps` drops the failure on
        // purpose, so an unwritten file here is the documented degraded path and not a bug.
        if let Ok(written) = fs::read_to_string(cgroup.dir().join("cpu.max")) {
            assert_eq!(written.trim(), "50000 100000");
        }
    }
}
