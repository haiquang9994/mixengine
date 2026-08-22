//! Building a request on disk the way the daemon would, and reading the answer.
//!
//! Shared by `protocol.rs`, which runs under whatever token the suite has, and `system.rs`, which
//! runs under an administrative one.

// Each integration test binary compiles this module separately, so anything `protocol.rs` uses and
// `system.rs` does not — `run_with`, `version` — is dead code in one of them. The same reason
// `crates/mixengine-cli/tests/harness/mod.rs` carries this line.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use mixengine_proto::privileged::PrivilegedResponse;
use tempfile::TempDir;

/// A home with a request directory in it, exactly where the daemon puts one.
///
/// The pieces are kept apart and assembled at [`Request::write`] rather than edited into a string as
/// they are set: a builder whose second call silently discarded the first would make a test that
/// looked right and asked something else.
pub(crate) struct Request {
    home: TempDir,
    directory: PathBuf,
    named_home: Option<PathBuf>,
    version: u32,
    nonce: String,
    ops: String,
    /// Whom to hand the files to before the helper reads them. See `owned_by_the_caller`.
    #[cfg(unix)]
    uid: Option<u32>,
}

/// What one invocation did.
pub(crate) struct Ran {
    /// The exit code, or `None` if a signal ended it.
    pub(crate) code: Option<i32>,
    /// The response, when one was written. **When this is `Some`, it is the answer** and the code
    /// says nothing.
    pub(crate) response: Option<PrivilegedResponse>,
    /// Whatever it complained about.
    pub(crate) stderr: String,
}

impl Default for Request {
    fn default() -> Self {
        Self::new()
    }
}

impl Request {
    /// A well-formed request carrying one `probe`, in a home this test owns.
    pub(crate) fn new() -> Self {
        let home = TempDir::new().expect("the system temporary directory is writable");
        let directory = home.path().join("run").join("elevate").join("one");
        std::fs::create_dir_all(&directory).expect("the request directory");

        Self {
            home,
            directory,
            named_home: None,
            version: 1,
            nonce: "n".to_owned(),
            ops: r#"[{ "op": "probe" }]"#.to_owned(),
            #[cfg(unix)]
            uid: None,
        }
    }

    /// Replace the operations, as raw JSON so a test can write one this build has never heard of.
    #[must_use]
    pub(crate) fn ops(mut self, ops: &str) -> Self {
        self.ops = ops.to_owned();
        self
    }

    /// Name a different `home` than the one the request actually sits in.
    #[must_use]
    pub(crate) fn home(mut self, home: &Path) -> Self {
        self.named_home = Some(home.to_path_buf());
        self
    }

    /// Name this request, so a test can tell its own lines out of a log every test appends to.
    #[must_use]
    pub(crate) fn nonce(mut self, nonce: &str) -> Self {
        self.nonce = nonce.to_owned();
        self
    }

    /// Claim a protocol version.
    #[must_use]
    pub(crate) fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// Hand the home and the request to whoever invoked `sudo`, so this looks like a request a daemon
    /// wrote rather than one root planted.
    ///
    /// A no-op on Windows, where an administrator's own files belong to `BUILTIN\Administrators` and
    /// that is the ordinary case rather than the refused one. On Unix without `sudo` — a suite run
    /// directly as root — there is nobody to hand it to, and the test that follows fails on exit 65,
    /// which is the correct answer to the request it managed to build.
    #[must_use]
    #[cfg_attr(
        windows,
        expect(
            unused_mut,
            reason = "the body that assigns through it is `#[cfg(unix)]`, and the signature is one                       thing rather than two"
        )
    )]
    pub(crate) fn owned_by_the_caller(mut self) -> Self {
        #[cfg(unix)]
        {
            self.uid = std::env::var("SUDO_UID")
                .ok()
                .and_then(|uid| uid.parse().ok());

            if let Some(uid) = self.uid {
                for path in [self.home.path(), self.directory.as_path()] {
                    std::os::unix::fs::chown(path, Some(uid), None)
                        .expect("root may give a file away");
                }
            }
        }

        self
    }

    /// The directory the request and its answer live in.
    #[must_use]
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Write it out and hand back its path.
    ///
    /// The `TempDir` is kept alive by `self`, so the caller has to keep the `Request` alive too — a
    /// `let path = Request::new().write();` would delete the home before the helper opened it.
    pub(crate) fn write(&self) -> PathBuf {
        let home = self
            .named_home
            .as_deref()
            .unwrap_or_else(|| self.home.path());
        let body = format!(
            r#"{{ "version": {}, "home": {}, "nonce": "{}", "ops": {} }}"#,
            self.version,
            serde_json::to_string(home).expect("a path encodes"),
            self.nonce,
            self.ops,
        );

        let path = self.directory.join("request.json");
        std::fs::write(&path, body).expect("the request");

        #[cfg(unix)]
        if let Some(uid) = self.uid {
            std::os::unix::fs::chown(&path, Some(uid), None).expect("root may give a file away");
        }

        path
    }
}

/// Run the helper against a request file.
pub(crate) fn run(request: &Path) -> Ran {
    let output = Command::new(env!("CARGO_BIN_EXE_mixengine-elevate"))
        .arg(request)
        .output()
        .expect("the binary under test was built by cargo");

    // `ok()` on the parse and not `expect`: one test plants a *non-JSON* answer beside the request
    // to prove the helper refuses rather than overwrites it, and that file is still sitting there
    // when this reads. `None` therefore means "no answer this harness can read", which is what
    // every assertion here is phrased against.
    let response = std::fs::read_to_string(request.with_file_name("response.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());

    Ran {
        code: output.status.code(),
        response,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Run it with whatever arguments a test chooses, including none.
pub(crate) fn run_with(arguments: &[&Path]) -> Ran {
    let output = Command::new(env!("CARGO_BIN_EXE_mixengine-elevate"))
        .args(arguments)
        .output()
        .expect("the binary under test was built by cargo");

    Ran {
        code: output.status.code(),
        response: None,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}
