//! Finding an installed desktop application in the registry — roadmap task **T83**, D4.
//!
//! App Paths first, because it is the documented mechanism; then the uninstall table, because
//! Tauri's NSIS installer — MixDB's — writes no App Paths entry (measured against the upstream
//! template and against this machine) and does write `DisplayIcon = "<dir>\<binary>.exe"`. The hint
//! is a file name in both places, compared case-insensitively, since NTFS does.
//!
//! Through `windows-sys` as `path.rs` reaches the registry, never through `reg.exe` or a scripting
//! host. Every handle is closed by [`Key`]'s drop.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE,
    REG_VALUE_TYPE, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW,
};

use crate::desktop::entry;
use crate::{DesktopApps, Error, InstalledApp, Located, Result, Started};

/// The documented table: one subkey per executable name, its default value the path.
const APP_PATHS: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths";

/// What Programs and Features reads: one subkey per install, `DisplayIcon` naming the binary.
const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// The same table for 32-bit installers on a 64-bit Windows.
const UNINSTALL_32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

/// A key name is at most 255 characters; the buffer is asked for in characters with the terminator.
const KEY_NAME_CAPACITY: usize = 256;

/// One of the two hives a program is registered in.
///
/// Its own enum rather than an `HKEY`: that is a raw pointer, which is neither `Send` nor `Sync`,
/// and a [`Host`](crate::Host) is both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hive {
    /// `HKEY_CURRENT_USER` — a per-user install, which is Tauri's default.
    CurrentUser,

    /// `HKEY_LOCAL_MACHINE` — an install for every account.
    LocalMachine,
}

impl Hive {
    fn handle(self) -> HKEY {
        match self {
            Self::CurrentUser => HKEY_CURRENT_USER,
            Self::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// A key to search under.
#[derive(Debug, Clone)]
pub(crate) struct Root {
    /// Which hive.
    pub(crate) hive: Hive,

    /// The path under it.
    pub(crate) path: String,
}

#[derive(Debug)]
pub(crate) struct Apps {
    app_paths: Vec<Root>,
    uninstall: Vec<Root>,
}

impl Apps {
    /// This machine's tables, current user first.
    pub(crate) fn of_this_machine() -> Self {
        let root = |hive, path: &str| Root {
            hive,
            path: path.to_owned(),
        };

        Self::under(
            vec![
                root(Hive::CurrentUser, APP_PATHS),
                root(Hive::LocalMachine, APP_PATHS),
            ],
            vec![
                root(Hive::CurrentUser, UNINSTALL),
                root(Hive::LocalMachine, UNINSTALL),
                root(Hive::LocalMachine, UNINSTALL_32),
            ],
        )
    }

    /// Exactly these roots.
    pub(crate) fn under(app_paths: Vec<Root>, uninstall: Vec<Root>) -> Self {
        Self {
            app_paths,
            uninstall,
        }
    }

    /// `App Paths\<hint>`'s default value, where it names a file that exists.
    fn in_app_paths(&self, hint: &str) -> Result<Option<PathBuf>> {
        for root in &self.app_paths {
            let Some(key) = open(
                root.hive.handle(),
                &format!(r"{}\{hint}", root.path),
                KEY_QUERY_VALUE,
            )?
            else {
                continue;
            };

            if let Some(value) = key.string("")? {
                let program = PathBuf::from(entry::unquoted(&value));
                if program.is_file() {
                    return Ok(Some(program));
                }
            }
        }

        Ok(None)
    }

    /// The first uninstall row whose `DisplayIcon` is a file named `hint`, or whose
    /// `InstallLocation` holds one.
    fn in_uninstall_table(&self, hint: &str) -> Result<Option<PathBuf>> {
        for root in &self.uninstall {
            let Some(table) = open(root.hive.handle(), &root.path, KEY_ENUMERATE_SUB_KEYS)? else {
                continue;
            };

            for name in table.subkeys()? {
                let Some(row) = open(
                    root.hive.handle(),
                    &format!(r"{}\{name}", root.path),
                    KEY_QUERY_VALUE,
                )?
                else {
                    continue;
                };

                if let Some(icon) = row.string("DisplayIcon")? {
                    let program = PathBuf::from(entry::unquoted(&icon));
                    if same_name(&program, hint) && program.is_file() {
                        return Ok(Some(program));
                    }
                }

                if let Some(location) = row.string("InstallLocation")? {
                    let program = PathBuf::from(entry::unquoted(&location)).join(hint);
                    if program.is_file() {
                        return Ok(Some(program));
                    }
                }
            }
        }

        Ok(None)
    }
}

impl DesktopApps for Apps {
    fn locate(&self, hint: &str) -> Result<Located> {
        let found = match self.in_app_paths(hint)? {
            Some(program) => Some(program),
            None => self.in_uninstall_table(hint)?,
        };

        if let Some(program) = found {
            return Ok(Located::Installed(InstalledApp {
                program,
                args: Vec::new(),
            }));
        }

        Ok(Located::NotInstalled {
            searched: format!(
                "{hint} under App Paths and in the uninstall table, for this user and this machine"
            ),
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

/// Whether `path`'s file name is `hint`, the way NTFS compares.
fn same_name(path: &Path, hint: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(hint))
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        #[expect(
            unsafe_code,
            reason = "the handle came from RegOpenKeyExW in this module and is not used after this"
        )]
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

/// Open `path` under `hive`, or [`None`] when it is not there.
fn open(hive: HKEY, path: &str, access: u32) -> Result<Option<Key>> {
    let subkey = wide(path);
    let mut handle: HKEY = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the one out-parameter is owned by this frame and is wrapped in `Key`, which \
                  closes it"
    )]
    let status = unsafe { RegOpenKeyExW(hive, subkey.as_ptr(), 0, access, &raw mut handle) };

    match status {
        ERROR_SUCCESS => Ok(Some(Key(handle))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        status => Err(os("open a registry key", status)),
    }
}

impl Key {
    /// A string value, or [`None`] when absent. `""` is the default value.
    ///
    /// The size is asked for first with a null buffer, `path.rs`'s arrangement: a path has no bound
    /// worth guessing at.
    fn string(&self, name: &str) -> Result<Option<String>> {
        let name = wide(name);
        let mut kind: REG_VALUE_TYPE = 0;
        let mut bytes: u32 = 0;

        #[expect(
            unsafe_code,
            reason = "the call writes only the two out-parameters, both owned by this frame"
        )]
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null(),
                &raw mut kind,
                std::ptr::null_mut(),
                &raw mut bytes,
            )
        };

        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(os("read a registry value", status));
        }

        let mut buffer = vec![0u16; bytes.div_ceil(2) as usize];
        let mut capacity = bytes;

        #[expect(
            unsafe_code,
            reason = "the buffer is sized by the query above and its capacity is passed alongside \
                      it, so the call cannot write past what this frame owns"
        )]
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null(),
                &raw mut kind,
                buffer.as_mut_ptr().cast::<u8>(),
                &raw mut capacity,
            )
        };

        if status != ERROR_SUCCESS {
            return Err(os("read a registry value", status));
        }

        let written = (capacity.div_ceil(2) as usize).min(buffer.len());
        let value = String::from_utf16_lossy(&buffer[..written]);

        Ok(Some(value.trim_end_matches('\0').to_owned()))
    }

    /// Every subkey's name.
    fn subkeys(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut index = 0u32;

        loop {
            let mut buffer = vec![0u16; KEY_NAME_CAPACITY];
            let mut length = u32::try_from(buffer.len()).expect("256 fits in a u32");

            #[expect(
                unsafe_code,
                reason = "the buffer and its length are this frame's; the other out-parameters \
                          are null and documented as optional"
            )]
            let status = unsafe {
                RegEnumKeyExW(
                    self.0,
                    index,
                    buffer.as_mut_ptr(),
                    &raw mut length,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };

            match status {
                ERROR_SUCCESS => {
                    let written = (length as usize).min(buffer.len());
                    names.push(String::from_utf16_lossy(&buffer[..written]));
                    index += 1;
                }
                ERROR_NO_MORE_ITEMS => return Ok(names),
                status => return Err(os("enumerate a registry key", status)),
            }
        }
    }
}

/// A NUL-terminated UTF-16 string, which is what every `W` entry point takes.
fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// A Win32 status as the error this crate reports.
fn os(action: &'static str, status: u32) -> Error {
    Error::Os {
        action,
        source: std::io::Error::from_raw_os_error(i32::try_from(status).unwrap_or(i32::MAX)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry subtree of this test's own, written through `reg.exe` and deleted the same way —
    /// `path.rs`'s `TestKey`, and for its reason: cleanup through a fourth registry entry point
    /// would be paying in the product for something no shipped path needs.
    struct TestTree(String);

    impl TestTree {
        fn new(name: &str) -> Self {
            Self(format!(r"Software\MixEngine\tests\desktop\{name}"))
        }

        fn reg(&self, args: &[&str]) {
            let mut command = std::process::Command::new("reg");
            command.args(args);
            crate::sys::command::without_a_window(&mut command);
            let output = command.output().expect("reg runs");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        fn set(&self, subkey: &str, value: &str, data: &str) {
            let key = format!(r"HKCU\{}\{subkey}", self.0);
            self.reg(&["add", &key, "/v", value, "/t", "REG_SZ", "/d", data, "/f"]);
        }

        fn set_default(&self, subkey: &str, data: &str) {
            let key = format!(r"HKCU\{}\{subkey}", self.0);
            self.reg(&["add", &key, "/ve", "/t", "REG_SZ", "/d", data, "/f"]);
        }

        fn root(&self, under: &str) -> Root {
            Root {
                hive: Hive::CurrentUser,
                path: format!(r"{}\{under}", self.0),
            }
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            let mut command = std::process::Command::new("reg");
            command.args(["delete", &format!(r"HKCU\{}", self.0), "/f"]);
            crate::sys::command::without_a_window(&mut command);
            let _ = command.output();
        }
    }

    fn a_program() -> tempfile::NamedTempFile {
        tempfile::Builder::new()
            .suffix(".exe")
            .tempfile()
            .expect("a program file")
    }

    fn name_of(program: &tempfile::NamedTempFile) -> String {
        program
            .path()
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn app_paths_answers_first() {
        let tree = TestTree::new("app_paths");
        let program = a_program();
        tree.set_default(
            r"App Paths\mixengine-test.exe",
            &format!("\"{}\"", program.path().display()),
        );
        let apps = Apps::under(vec![tree.root("App Paths")], vec![]);

        match apps.locate("MixEngine-Test.exe").expect("answers") {
            Located::Installed(app) => assert_eq!(app.program, program.path()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_uninstall_table_is_matched_on_display_icon_case_insensitively() {
        let tree = TestTree::new("uninstall");
        let program = a_program();
        tree.set(
            r"Uninstall\Something Else",
            "DisplayIcon",
            r"C:\nowhere\other.exe,0",
        );
        tree.set(
            r"Uninstall\MixDB",
            "DisplayIcon",
            &format!("\"{}\",0", program.path().display()),
        );
        let apps = Apps::under(vec![], vec![tree.root("Uninstall")]);

        match apps
            .locate(&name_of(&program).to_uppercase())
            .expect("answers")
        {
            Located::Installed(app) => assert_eq!(app.program, program.path()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn install_location_is_the_fallback_when_there_is_no_icon() {
        let tree = TestTree::new("location");
        let program = a_program();
        let dir = program.path().parent().expect("a dir");
        tree.set(
            r"Uninstall\MixDB",
            "InstallLocation",
            &format!("\"{}\"", dir.display()),
        );
        let apps = Apps::under(vec![], vec![tree.root("Uninstall")]);

        assert!(matches!(
            apps.locate(&name_of(&program)).expect("answers"),
            Located::Installed(_)
        ));
    }

    #[test]
    fn nothing_matching_says_where_it_looked() {
        let tree = TestTree::new("absent");
        tree.set(r"Uninstall\MixDB", "DisplayIcon", r"C:\nowhere\mixdb.exe");
        let apps = Apps::under(vec![tree.root("App Paths")], vec![tree.root("Uninstall")]);

        match apps.locate("mixdb.exe").expect("answers") {
            Located::NotInstalled { searched } => {
                assert!(searched.contains("App Paths"), "{searched}");
            }
            other => panic!("{other:?}"),
        }
    }
}
