//! Finding an installed application by bundle identifier — roadmap task **T83**, D4.
//!
//! `mdfind` asks Spotlight, which is what Launch Services itself consults; the query is a literal
//! because the identifier is held to letters, digits, dots and dashes before it is spliced in. The
//! executable's name is read with `defaults read`, since an `Info.plist` may be binary. Both tools
//! are named absolutely so the daemon's `PATH` cannot decide what runs.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{DesktopApps, Error, InstalledApp, Located, Result, Started};

/// Spotlight's command line, part of the base system.
const MDFIND: &str = "/usr/bin/mdfind";

/// The property-list reader, part of the base system.
const DEFAULTS: &str = "/usr/bin/defaults";

#[derive(Debug)]
pub(crate) struct Apps {
    /// `$HOME`, for `~/Applications`.
    home: Option<PathBuf>,
}

impl Apps {
    pub(crate) fn of_this_user() -> Self {
        Self {
            home: std::env::var_os("HOME").map(PathBuf::from),
        }
    }
}

impl DesktopApps for Apps {
    fn locate(&self, hint: &str) -> Result<Located> {
        if !is_bundle_id(hint) {
            return Ok(Located::NotInstalled {
                searched: format!("nothing — `{hint}` is not a bundle identifier"),
            });
        }

        let listed = run(MDFIND, &[&format!("kMDItemCFBundleIdentifier == '{hint}'")])?;
        let found: Vec<PathBuf> = listed
            .lines()
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect();

        let Some(bundle) = choose(&found, self.home.as_deref()) else {
            return Ok(Located::NotInstalled {
                searched: format!("{hint} through Spotlight, in /Applications and ~/Applications"),
            });
        };

        let info = bundle.join("Contents").join("Info");
        let name = run(
            DEFAULTS,
            &["read", &info.display().to_string(), "CFBundleExecutable"],
        )?;
        let program = bundle.join("Contents").join("MacOS").join(name.trim());

        if !program.is_file() {
            return Ok(Located::NotInstalled {
                searched: format!(
                    "{hint} through Spotlight — {} names no executable",
                    bundle.display()
                ),
            });
        }

        Ok(Located::Installed(InstalledApp {
            program,
            args: Vec::new(),
        }))
    }

    fn launch(
        &self,
        app: &InstalledApp,
        args: &[OsString],
        env: &BTreeMap<String, String>,
    ) -> Result<Started> {
        crate::desktop::launch(app, args, env)
    }
}

/// Letters, digits, dots and dashes — what a bundle identifier is made of, and what keeps the
/// Spotlight query a literal.
fn is_bundle_id(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// One bundle out of what Spotlight answered: `/Applications`, then `~/Applications`, then anything
/// not in a Trash.
fn choose(found: &[PathBuf], home: Option<&Path>) -> Option<PathBuf> {
    let in_trash = |path: &&PathBuf| path.components().any(|part| part.as_os_str() == ".Trash");
    let candidates: Vec<&PathBuf> = found.iter().filter(|path| !in_trash(path)).collect();

    let preferred = |prefix: PathBuf| {
        candidates
            .iter()
            .find(|path| path.starts_with(&prefix))
            .map(|path| (*path).clone())
    };

    preferred(PathBuf::from("/Applications"))
        .or_else(|| home.and_then(|home| preferred(home.join("Applications"))))
        .or_else(|| candidates.first().map(|path| (*path).clone()))
}

/// Run a base-system tool and hand back its stdout.
fn run(program: &'static str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|source| Error::Command {
            command: program,
            path: None,
            status: "could not be started".to_owned(),
            output: source.to_string(),
        })?;

    if !output.status.success() {
        return Err(Error::Command {
            command: program,
            path: None,
            status: output.status.to_string(),
            output: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_id_is_letters_digits_dots_and_dashes() {
        assert!(is_bundle_id("io.github.haiquang9994.mixdb"));
        assert!(is_bundle_id("nz.mix-db.app"));
        assert!(!is_bundle_id("nz.mix'db"));
        assert!(!is_bundle_id("nz mix"));
        assert!(!is_bundle_id(""));
    }

    #[test]
    fn applications_win_and_the_trash_never_does() {
        let home = Path::new("/Users/me");
        let found = [
            PathBuf::from("/Users/me/.Trash/MixDB.app"),
            PathBuf::from("/Users/me/Downloads/MixDB.app"),
            PathBuf::from("/Applications/MixDB.app"),
        ];
        assert_eq!(
            choose(&found, Some(home)),
            Some(PathBuf::from("/Applications/MixDB.app"))
        );

        let only_downloads = [
            PathBuf::from("/Users/me/.Trash/MixDB.app"),
            PathBuf::from("/Users/me/Downloads/MixDB.app"),
        ];
        assert_eq!(
            choose(&only_downloads, Some(home)),
            Some(PathBuf::from("/Users/me/Downloads/MixDB.app"))
        );

        let only_trash = [PathBuf::from("/Users/me/.Trash/MixDB.app")];
        assert_eq!(choose(&only_trash, Some(home)), None);
    }
}
