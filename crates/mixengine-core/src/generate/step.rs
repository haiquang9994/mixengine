//! One program a piece of work runs, with a deadline and possibly a credential.
//!
//! Lifted out of [`first_run`](super::first_run) when a second kind of work needed the same shape —
//! roadmap task **T77a**. A bootstrap and a database provisioning have nothing else in common, and
//! the thing they do share is not "what has to happen once before a service is ever started", which
//! is what the module it used to live in is named after.
//!
//! Both hand-written [`Debug`] impls came with it, and they are why this is a file rather than a
//! pair of struct definitions: a step may hold a generated password on its standard input, and
//! `.claude/standards/rust.md` says a type that *might* hold a secret redacts it rather than
//! trusting every caller that ever writes `{:?}`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use mixengine_proto::Millis;

/// One program a piece of work runs.
pub struct Step {
    /// What the progress line says: `creating the data directory`.
    pub label: String,

    /// The program. Absolute, for [`ServiceSpec`](mixengine_proto::ServiceSpec)'s reason: a relative
    /// one is whatever the `PATH` happens to say at the moment it runs.
    pub program: PathBuf,

    /// What to pass it.
    pub args: Vec<String>,

    /// A file this step needs to exist while it runs, and never a moment longer.
    ///
    /// **MySQL is why this exists** — roadmap task **T34c**. MariaDB sets its root password through
    /// `mariadbd --bootstrap`, which reads SQL on standard input; MySQL removed `--bootstrap` at
    /// 5.7.6 and offers `--init-file` instead, which is a *path*. The three ways to get a statement
    /// carrying a generated password into that server are a file, an argument list every process on
    /// the machine can read, or a temporary server on a port anybody can connect to — and the file
    /// is the only one whose exposure is bounded by something we control.
    ///
    /// So the daemon writes it as narrowly as the OS allows, runs the step, and removes it —
    /// whether the step succeeded, failed or ran out of time. A recipe never touches the disk.
    pub secret_file: Option<SecretFile>,

    /// Fed to the program's standard input, which is then closed.
    ///
    /// This is how SQL reaches `mariadbd --bootstrap` without a temporary file — which for a
    /// statement that sets a root password would be a plaintext credential on disk.
    pub stdin: Option<String>,

    /// The whole environment, over the platform's own floor.
    pub env: BTreeMap<String, String>,

    /// Where it runs.
    pub cwd: PathBuf,

    /// How long it is given before it is killed and the ritual has failed.
    pub timeout: Millis,
}

/// A file that carries a credential, written for one step and removed after it.
///
/// Its own type rather than a pair, so that [`Step`]'s hand-written [`Debug`](fmt::Debug) has
/// somewhere to be careful: the path is what a reader debugging a bootstrap needs and the content is
/// what must never reach `daemon.log`.
pub struct SecretFile {
    /// Where it goes. Inside the home, and named by the service it is for.
    pub path: PathBuf,

    /// What it holds — a SQL statement with a generated password in it.
    pub content: String,
}

/// The content is never printed. See [`Step`]'s own reasoning.
impl fmt::Debug for SecretFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretFile")
            .field("path", &self.path)
            .field("content", &format!("<{} bytes>", self.content.len()))
            .finish()
    }
}

/// Written by hand, and [`Step::stdin`] is the reason.
///
/// It carries a generated password. `.claude/standards/rust.md`'s rule is that a struct which
/// *might* hold a secret redacts it rather than trusting every caller that ever writes `{:?}`, and a
/// `tracing` field on a step that failed is one line away at all times. The length stays, because it
/// is what a reader debugging a bootstrap actually needs.
impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Step")
            .field("label", &self.label)
            .field("program", &self.program)
            .field("args", &self.args)
            .field(
                "stdin",
                &self
                    .stdin
                    .as_ref()
                    .map(|input| format!("<{} bytes>", input.len())),
            )
            .field("secret_file", &self.secret_file)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .field("cwd", &self.cwd)
            .field("timeout", &self.timeout)
            .finish()
    }
}
