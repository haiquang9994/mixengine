//! Finding an installed desktop application through its desktop entry — roadmap task **T83**, D4.
//!
//! `$XDG_DATA_HOME/applications` first, then each `$XDG_DATA_DIRS` entry's `applications/`, which is
//! the search every launcher performs and the order the specification gives. `TryExec=` names the
//! program where present; otherwise the first word of `Exec=` with its field codes removed. A bare
//! name resolves on this process's `PATH`, as a launcher would resolve it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::desktop::entry;
use crate::{DesktopApps, InstalledApp, Located, Result, Started};

/// The data directories whose `applications/` are searched, in order.
#[derive(Debug)]
pub(crate) struct Apps {
    data_dirs: Vec<PathBuf>,
}

impl Apps {
    /// This user's directories, from the environment and the specification's defaults.
    pub(crate) fn of_this_user() -> Self {
        let mut dirs = Vec::new();

        match std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            Some(home) => dirs.push(PathBuf::from(home)),
            None => {
                if let Some(home) = std::env::var_os("HOME") {
                    dirs.push(PathBuf::from(home).join(".local").join("share"));
                }
            }
        }

        match std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty()) {
            Some(list) => dirs.extend(std::env::split_paths(&list)),
            None => dirs.extend([
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]),
        }

        Self::with_dirs(dirs)
    }

    /// Exactly these directories.
    pub(crate) fn with_dirs(data_dirs: Vec<PathBuf>) -> Self {
        Self { data_dirs }
    }

    /// Where a person is told this looked.
    fn searched(&self) -> String {
        self.data_dirs
            .iter()
            .map(|dir| dir.join("applications").display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl DesktopApps for Apps {
    fn locate(&self, hint: &str) -> Result<Located> {
        for dir in &self.data_dirs {
            let file = dir.join("applications").join(hint);
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };

            if let Some(app) = read(&text) {
                return Ok(Located::Installed(app));
            }
        }

        Ok(Located::NotInstalled {
            searched: format!("{hint} in {}", self.searched()),
        })
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

/// The application a desktop entry starts, when its program is on this machine.
fn read(text: &str) -> Option<InstalledApp> {
    let mut try_exec = None;
    let mut exec = None;

    for line in text.lines() {
        if let Some(value) = line.strip_prefix("TryExec=") {
            try_exec = Some(value.trim().to_owned());
        } else if let Some(value) = line.strip_prefix("Exec=") {
            exec = entry::exec_line(value);
        }
    }

    let (program, args) = match (try_exec, exec) {
        (Some(program), Some((_, args))) => (program, args),
        (Some(program), None) => (program, Vec::new()),
        (None, Some(parsed)) => parsed,
        (None, None) => return None,
    };

    let program = resolve(Path::new(&program))?;

    Some(InstalledApp {
        program,
        args: args.into_iter().map(OsString::from).collect(),
    })
}

/// An absolute path that exists, or a name found on `PATH`.
fn resolve(program: &Path) -> Option<PathBuf> {
    if program.is_absolute() {
        return program.is_file().then(|| program.to_path_buf());
    }

    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs_with(entry: Option<(&str, &str)>) -> (tempfile::TempDir, Apps) {
        let temp = tempfile::tempdir().expect("a temporary data dir");
        let applications = temp.path().join("applications");
        std::fs::create_dir_all(&applications).expect("the applications dir");
        if let Some((name, body)) = entry {
            std::fs::write(applications.join(name), body).expect("a desktop entry");
        }
        let apps = Apps::with_dirs(vec![temp.path().to_path_buf()]);
        (temp, apps)
    }

    #[test]
    fn an_absent_entry_says_where_it_looked() {
        let (temp, apps) = dirs_with(None);
        match apps.locate("mixdb.desktop").expect("answers") {
            Located::NotInstalled { searched } => {
                let applications = temp.path().join("applications").display().to_string();
                assert!(searched.contains(&applications), "{searched}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_entry_with_an_absolute_exec_is_found_with_its_fixed_arguments() {
        let program = tempfile::NamedTempFile::new().expect("a program");
        let body = format!(
            "[Desktop Entry]\nName=MixDB\nExec=\"{}\" --flag %U\nType=Application\n",
            program.path().display()
        );
        let (_temp, apps) = dirs_with(Some(("mixdb.desktop", &body)));

        match apps.locate("mixdb.desktop").expect("answers") {
            Located::Installed(app) => {
                assert_eq!(app.program, program.path());
                assert_eq!(app.args, vec![OsString::from("--flag")]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn try_exec_wins_and_a_bare_name_resolves_on_the_path() {
        let body = "[Desktop Entry]\nTryExec=sh\nExec=mixdb %u\n";
        let (_temp, apps) = dirs_with(Some(("mixdb.desktop", body)));

        match apps.locate("mixdb.desktop").expect("answers") {
            Located::Installed(app) => assert!(
                app.program.is_absolute() && app.program.ends_with("sh"),
                "{app:?}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_entry_whose_program_is_gone_is_not_installed() {
        let body = "[Desktop Entry]\nExec=/nonexistent/mixengine-nothing %u\n";
        let (_temp, apps) = dirs_with(Some(("mixdb.desktop", body)));
        assert!(matches!(
            apps.locate("mixdb.desktop").expect("answers"),
            Located::NotInstalled { .. }
        ));
    }
}
