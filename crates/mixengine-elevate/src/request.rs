//! Reading a request, and refusing one that cannot be trusted.
//!
//! Every refusal here is a refusal of the **whole request**: exit 65, and no response file. That is
//! the distinction the exit codes turn on — a bad operation inside a good request is exit 0 with an
//! outcome at that operation's index, and never reaches this module.
//!
//! **The identity of the caller is the owner of the file.** The daemon runs as the user, and if the
//! daemon is compromised it is the attacker, so nothing this document asserts about who is asking is
//! believed. The daemon wrote the file while running as the user, so the filesystem already knows.

use std::fmt;
use std::path::{Path, PathBuf};

use mixengine_platform::elevated::{self, Owner};
use mixengine_platform::paths::in_full;
use mixengine_proto::PROTOCOL_VERSION;
use mixengine_proto::privileged::{PrivilegedRequest, RESPONSE_FILE_NAME};

/// A request that has passed every check, and where its answer goes.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) struct Accepted {
    /// The document, with its operations still undecoded.
    pub(crate) request: PrivilegedRequest,
    /// `response.json`, beside the request. Not yet created.
    pub(crate) response: PathBuf,
    /// Whoever wrote the request, taken from the file rather than from the document.
    pub(crate) caller: Owner,
}

/// Why the whole request was refused, phrased for whoever reads stderr — which, this being a process
/// nobody watches, means a developer reading a daemon log after the fact.
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) struct Rejected(String);

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Read the request at `path`, or say why it will not be honoured.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into main.rs in Task 7 of this series")
)]
pub(crate) fn read(path: &Path) -> Result<Accepted, Rejected> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|source| Rejected(format!("cannot read {}: {source}", path.display())))?;

    // Before anything is read through it: a symlink is somebody else choosing which file root opens,
    // after root has decided to trust the name.
    if metadata.file_type().is_symlink() {
        return Err(Rejected(format!("{} is a symlink", path.display())));
    }

    if !metadata.is_file() {
        return Err(Rejected(format!("{} is not a file", path.display())));
    }

    let caller = elevated::owner_of(path).map_err(|error| Rejected(error.to_string()))?;

    // The daemon never runs as root, so a request that belongs to root was not written by one. On
    // Windows this means SYSTEM alone — an administrator's own files belong to
    // `BUILTIN\Administrators`, and most Windows users are administrators.
    if caller.is_superuser() {
        return Err(Rejected(format!(
            "{} belongs to {caller}, which no daemon runs as",
            path.display()
        )));
    }

    if elevated::others_can_write(path).map_err(|error| Rejected(error.to_string()))? {
        return Err(Rejected(format!(
            "{} can be written by somebody other than {caller}",
            path.display()
        )));
    }

    // D10: the existence of the answer is the anti-replay check. No clock, no nonce store, no state
    // that outlives the process — the daemon writes each request into a fresh single-use directory,
    // so this is the only thing that ever appears beside it. `symlink_metadata` rather than
    // `exists`, which answers `false` for a dangling link somebody planted.
    let response = path.with_file_name(RESPONSE_FILE_NAME);
    if std::fs::symlink_metadata(&response).is_ok() {
        return Err(Rejected(format!(
            "{} has already been answered",
            path.display()
        )));
    }

    let text = std::fs::read_to_string(path)
        .map_err(|source| Rejected(format!("cannot read {}: {source}", path.display())))?;

    let request: PrivilegedRequest = serde_json::from_str(&text)
        .map_err(|source| Rejected(format!("cannot parse {}: {source}", path.display())))?;

    if request.version != PROTOCOL_VERSION {
        return Err(Rejected(format!(
            "this helper speaks {PROTOCOL_VERSION} and the request is {}",
            request.version
        )));
    }

    // An empty batch asks for nothing, and giving it a meaning of its own would be a second way to
    // ask for the report that already arrives with every answer.
    if request.ops.is_empty() {
        return Err(Rejected("the request carries no operations".to_owned()));
    }

    let home = in_full(&request.home);
    let home_metadata = std::fs::symlink_metadata(&home)
        .map_err(|source| Rejected(format!("cannot read {}: {source}", home.display())))?;

    if home_metadata.file_type().is_symlink() || !home_metadata.is_dir() {
        return Err(Rejected(format!("{} is not a directory", home.display())));
    }

    let owner = elevated::owner_of(&home).map_err(|error| Rejected(error.to_string()))?;
    if owner != caller {
        return Err(Rejected(format!(
            "{caller} does not own {}, which the request calls its home",
            home.display()
        )));
    }

    // Both sides spelled the way the filesystem spells them, which is what `in_full` is for: `/tmp`
    // on macOS is a symlink to `/private/tmp` and `getcwd` answers with the second while a person
    // types the first.
    if !in_full(path).starts_with(&home) {
        return Err(Rejected(format!(
            "{} is not inside {}",
            path.display(),
            home.display()
        )));
    }

    Ok(Accepted {
        request,
        response,
        caller,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A home with a request in it, exactly where the daemon puts one: `run/elevate/<id>/`.
    fn a_request(body: &str) -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::TempDir::new().expect("a temporary directory");
        let directory = home.path().join("run").join("elevate").join("one");
        std::fs::create_dir_all(&directory).expect("the request directory");

        let path = directory.join("request.json");
        std::fs::write(&path, body).expect("the request");

        (home, path)
    }

    /// The body a daemon would write, with `home` filled in.
    fn body(home: &Path, ops: &str) -> String {
        format!(
            r#"{{ "version": 1, "home": {}, "nonce": "n", "ops": {ops} }}"#,
            serde_json::to_string(home).expect("a path encodes")
        )
    }

    #[test]
    fn a_well_formed_request_is_accepted() {
        let (home, path) = a_request("");
        std::fs::write(&path, body(home.path(), r#"[{ "op": "probe" }]"#)).unwrap();

        let accepted = read(&path).expect("a request the daemon would write");

        assert_eq!(accepted.request.ops.len(), 1);
        assert_eq!(accepted.request.nonce, "n");
        assert_eq!(accepted.response.file_name().unwrap(), "response.json");
        assert_eq!(accepted.response.parent(), path.parent());
        assert_eq!(
            accepted.caller,
            elevated::owner_of(&path).unwrap(),
            "D4: the caller is the file's owner, never anything the document said about it"
        );
    }

    #[test]
    fn a_request_that_is_not_there_is_refused() {
        let home = tempfile::TempDir::new().unwrap();

        let rejected = read(&home.path().join("never-written.json")).unwrap_err();

        assert!(rejected.to_string().contains("never-written"), "{rejected}");
    }

    #[test]
    fn a_request_that_is_not_json_is_refused() {
        let (home, path) = a_request("not json at all");
        let _ = home;

        assert!(read(&path).is_err());
    }

    #[test]
    fn a_protocol_this_build_does_not_know_is_refused() {
        let (home, path) = a_request("");
        std::fs::write(
            &path,
            format!(
                r#"{{ "version": 99, "home": {}, "nonce": "n", "ops": [{{ "op": "probe" }}] }}"#,
                serde_json::to_string(home.path()).unwrap()
            ),
        )
        .unwrap();

        let rejected = read(&path).unwrap_err();

        assert!(rejected.to_string().contains("99"), "{rejected}");
    }

    /// An empty batch asks for nothing. Giving it a meaning of its own — "just report" — is what the
    /// response header is already for, on every answer.
    #[test]
    fn a_request_with_no_operations_is_refused() {
        let (home, path) = a_request("");
        std::fs::write(&path, body(home.path(), "[]")).unwrap();

        assert!(read(&path).is_err());
    }

    /// D10: a processed request cannot be processed twice, and the check costs one `stat`.
    #[test]
    fn a_request_that_already_has_an_answer_is_refused() {
        let (home, path) = a_request("");
        std::fs::write(&path, body(home.path(), r#"[{ "op": "probe" }]"#)).unwrap();
        std::fs::write(path.with_file_name("response.json"), "{}").unwrap();

        let rejected = read(&path).unwrap_err();

        assert!(rejected.to_string().contains("already"), "{rejected}");
    }

    /// D4: without this, `--home C:\Windows\System32` is an escalation for every operation that
    /// takes a path. `/` and `C:\Windows` belong to an account this process is not.
    #[test]
    fn a_home_this_caller_does_not_own_is_refused() {
        let (home, path) = a_request("");
        let elsewhere = if cfg!(windows) { r"C:\Windows" } else { "/" };
        std::fs::write(
            &path,
            format!(
                r#"{{ "version": 1, "home": {}, "nonce": "n", "ops": [{{ "op": "probe" }}] }}"#,
                serde_json::to_string(elsewhere).unwrap()
            ),
        )
        .unwrap();
        let _ = home;

        let rejected = read(&path).unwrap_err();

        assert!(rejected.to_string().contains("own"), "{rejected}");
    }

    /// The request has to lie inside the home it names, or the home constrains nothing.
    #[test]
    fn a_request_outside_the_home_it_names_is_refused() {
        let (home, path) = a_request("");
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(&path, body(outside.path(), r#"[{ "op": "probe" }]"#)).unwrap();
        let _ = home;

        let rejected = read(&path).unwrap_err();

        assert!(rejected.to_string().contains("inside"), "{rejected}");
    }

    /// A symlink is somebody else choosing which file root reads, after root decided to trust it.
    #[cfg(unix)]
    #[test]
    fn a_request_that_is_a_symlink_is_refused() {
        let (home, path) = a_request("");
        std::fs::write(&path, body(home.path(), r#"[{ "op": "probe" }]"#)).unwrap();

        let link = path.with_file_name("linked.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();

        let rejected = read(&link).unwrap_err();

        assert!(rejected.to_string().contains("symlink"), "{rejected}");
    }

    #[cfg(unix)]
    #[test]
    fn a_request_anybody_can_rewrite_is_refused() {
        use std::os::unix::fs::PermissionsExt as _;

        let (home, path) = a_request("");
        std::fs::write(&path, body(home.path(), r#"[{ "op": "probe" }]"#)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();

        let rejected = read(&path).unwrap_err();

        assert!(rejected.to_string().contains("written"), "{rejected}");
    }
}
