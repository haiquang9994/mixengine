//! This user's `Path`, in `HKEY_CURRENT_USER\Environment`.
//!
//! Windows keeps the environment a new process inherits in the registry, in two places: a
//! machine-wide one under `HKLM` and a per-user one under `HKCU`. **Only the second is ever
//! touched.** It needs no elevation, it belongs to the account MixEngine runs as, and the machine
//! one is shared with every other user on the box — a development tool has no business in it.
//!
//! What that costs is stated rather than hidden: `explorer` builds a process's `PATH` as the machine
//! value followed by the user one, so a PHP installed for the whole machine is *ahead* of
//! `<root>/bin` no matter what this writes. Prepending inside the value we own is as far as a
//! user-level change reaches; `mix doctor` (T47) is where "something else is answering `php`"
//! belongs.
//!
//! # Why the API rather than `setx`
//!
//! `.claude/architecture/platform-abstraction.md`, rule 5: use the Windows API where there is one.
//! `setx` would do the job and would also **truncate the value at 1024 characters**, which is a
//! documented limit of that tool and not of the registry — on a developer's machine a user `Path`
//! past a kilobyte is ordinary, and the failure mode is losing the second half of somebody's PATH
//! for good. `RegSetValueExW` has no such limit.
//!
//! # The two things that are preserved rather than rewritten
//!
//! **The value's type.** A user `Path` is normally `REG_EXPAND_SZ`, which is what makes
//! `%USERPROFILE%\go\bin` mean something; reading one and writing it back as `REG_SZ` would turn
//! every such entry into a directory that does not exist. So the type that was there is the type
//! that goes back, and `REG_EXPAND_SZ` is used only when there was no value at all.
//!
//! **Every entry that is not ours.** The value is split, ours is inserted or dropped, and the rest
//! is joined back verbatim — empty segments and trailing semicolons included. Tidying somebody's
//! `PATH` is not what they asked for, and a "cleanup" that drops an entry it thought was redundant
//! is the one bug nobody would ever attribute to this.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ,
    REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
};

use crate::{Error, PathIntegration, PathLocation, PathState, Result};

/// The subkey of `HKEY_CURRENT_USER` that holds this user's environment.
const ENVIRONMENT: &str = "Environment";

/// The value inside it. Spelled as Windows spells it; the lookup is case-insensitive regardless.
const PATH: &str = "Path";

/// How long the "the environment changed" broadcast waits for one window to answer.
///
/// A ceiling and not a pause: `SMTO_ABORTIFHUNG` returns at once from a window that is not pumping
/// messages, and this only bounds the ones that are merely slow. The registry is already written by
/// the time it is sent, so every way this can go is "some program keeps its old PATH until it is
/// restarted" — which is what happens with no broadcast at all.
const BROADCAST_TIMEOUT_MS: u32 = 2_000;

/// This user's `Path`, and the key it lives in.
#[derive(Debug)]
pub(crate) struct Env {
    /// Which subkey of `HKEY_CURRENT_USER` to read and write.
    ///
    /// A field rather than the constant, so the module's own tests can exercise the real registry
    /// calls — the size query, the type round trip, the atomicity of one `RegSetValueExW` — against
    /// a key they create and delete, instead of against the value that decides whether the person
    /// running them can find `git` tomorrow.
    key: &'static str,
}

impl Env {
    /// The environment of the user this process runs as.
    pub(crate) fn of_this_user() -> Self {
        Self { key: ENVIRONMENT }
    }

    /// The `Path` value as it stands, with the type it is stored under.
    ///
    /// [`None`] when the key or the value is absent, which is an ordinary state: a fresh Windows
    /// account has no user `Path` at all until something writes one.
    fn read(&self) -> Result<Option<(REG_VALUE_TYPE, String)>> {
        let Some(key) = self.open()? else {
            return Ok(None);
        };

        let name = wide(PATH);
        let mut kind: REG_VALUE_TYPE = 0;
        let mut bytes: u32 = 0;

        // Asked for the size first, with a null buffer, because a `Path` has no bound worth
        // guessing at — the whole reason `setx` is not used here is that a real one goes past a
        // kilobyte.
        #[expect(
            unsafe_code,
            reason = "the registry has no safe binding in this tree; the call writes only the two \
                      out-parameters below, both owned by this frame"
        )]
        let status = unsafe {
            RegQueryValueExW(
                key.0,
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
            return Err(os("read this user's PATH", status));
        }

        // Rounded up: the registry counts bytes and this buffer counts `u16`s, and a value whose
        // length is odd — which nothing writes, but which nothing forbids either — must not lose
        // its last byte.
        let mut buffer = vec![0u16; bytes.div_ceil(2) as usize];
        let mut capacity = bytes;

        #[expect(
            unsafe_code,
            reason = "the buffer is sized by the query above and its capacity is passed alongside \
                      it, so the call cannot write past what this frame owns"
        )]
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                name.as_ptr(),
                std::ptr::null(),
                &raw mut kind,
                buffer.as_mut_ptr().cast::<u8>(),
                &raw mut capacity,
            )
        };

        if status != ERROR_SUCCESS {
            return Err(os("read this user's PATH", status));
        }

        // The registry stores the terminator; a value that grew between the two calls is bounded by
        // what was actually written back into `capacity`.
        let written = (capacity.div_ceil(2) as usize).min(buffer.len());
        let value = String::from_utf16_lossy(&buffer[..written]);

        Ok(Some((kind, value.trim_end_matches('\0').to_owned())))
    }

    /// Write `value` back under `kind`, creating the key if this is a home that has never had one.
    fn write(&self, kind: REG_VALUE_TYPE, value: &str) -> Result<()> {
        let key = self.create()?;
        let name = wide(PATH);
        let data = wide(value);

        #[expect(
            unsafe_code,
            reason = "the length passed is the buffer's own, in bytes, and the call only reads it"
        )]
        let status = unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                kind,
                data.as_ptr().cast::<u8>(),
                u32::try_from(std::mem::size_of_val(data.as_slice()))
                    .expect("a PATH shorter than four gigabytes"),
            )
        };

        if status != ERROR_SUCCESS {
            return Err(os("write this user's PATH", status));
        }

        drop(key);
        self.announce();

        Ok(())
    }

    /// Tell every window that the environment changed, so a `cmd` started from Explorer afterwards
    /// picks the new value up without a logout.
    ///
    /// **Never fails the operation.** The registry is already written; a desktop that did not hear
    /// about it is a desktop where the change applies to processes started after the next logon,
    /// which is exactly what happens on a machine with no Explorer running at all — a service
    /// account, a Server Core box, an SSH session.
    fn announce(&self) {
        // Only for the real key. A test pointing this at a key of its own has no business telling
        // every window on the machine to re-read an environment that did not change.
        if self.key != ENVIRONMENT {
            return;
        }

        let subject = wide(ENVIRONMENT);
        let mut answered: usize = 0;

        #[expect(
            unsafe_code,
            reason = "SendMessageTimeoutW only reads the string and writes the one out-parameter; \
                      the result is deliberately discarded — see this function's own note"
        )]
        let _ = unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                subject.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                BROADCAST_TIMEOUT_MS,
                &raw mut answered,
            )
        };
    }

    /// Open the key for reading, or [`None`] when it is not there.
    fn open(&self) -> Result<Option<Key>> {
        let subkey = wide(self.key);
        let mut handle: HKEY = std::ptr::null_mut();

        #[expect(
            unsafe_code,
            reason = "the one out-parameter is owned by this frame and is wrapped in `Key`, which \
                      closes it"
        )]
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &raw mut handle,
            )
        };

        match status {
            ERROR_SUCCESS => Ok(Some(Key(handle))),
            ERROR_FILE_NOT_FOUND => Ok(None),
            status => Err(os("open this user's environment", status)),
        }
    }

    /// Open the key for writing, creating it when it does not exist.
    fn create(&self) -> Result<Key> {
        let subkey = wide(self.key);
        let mut handle: HKEY = std::ptr::null_mut();

        #[expect(
            unsafe_code,
            reason = "as `open`, plus a null class and null security attributes, both of which mean \
                      'the defaults' rather than pointing at anything"
        )]
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &raw mut handle,
                std::ptr::null_mut(),
            )
        };

        match status {
            ERROR_SUCCESS => Ok(Key(handle)),
            status => Err(os("open this user's environment", status)),
        }
    }

    /// The one location this OS has, described for a person.
    fn location(&self, present: bool, changed: bool) -> PathLocation {
        PathLocation {
            name: format!(r"HKEY_CURRENT_USER\{}\{PATH}", self.key),
            present,
            changed,
        }
    }
}

impl PathIntegration for Env {
    fn add(&self, dir: &Path) -> Result<PathState> {
        let dir = dir.display().to_string();
        let current = self.read()?;

        let (kind, value) = match &current {
            Some((kind, value)) => (*kind, value.as_str()),
            // A fresh account with no user `Path` at all. `REG_EXPAND_SZ` is what Windows itself
            // creates one as, and it is the type that keeps a `%USERPROFILE%` somebody adds later
            // working.
            None => (REG_EXPAND_SZ, ""),
        };

        if entries(value).any(|entry| same(entry, &dir)) {
            return Ok(PathState {
                locations: vec![self.location(true, false)],
            });
        }

        // Prepended, so that `<root>/bin` beats another PHP in this user's own PATH. What it cannot
        // beat is the machine value, which comes first in the environment Windows composes — see
        // this module's note.
        let wanted = match value.is_empty() {
            true => dir,
            false => format!("{dir};{value}"),
        };

        self.write(kind, &wanted)?;

        Ok(PathState {
            locations: vec![self.location(true, true)],
        })
    }

    fn remove(&self, dir: &Path) -> Result<PathState> {
        let dir = dir.display().to_string();

        let Some((kind, value)) = self.read()? else {
            return Ok(PathState {
                locations: vec![self.location(false, false)],
            });
        };

        let kept: Vec<&str> = entries(&value).filter(|entry| !same(entry, &dir)).collect();
        let wanted = kept.join(";");

        if wanted == value {
            return Ok(PathState {
                locations: vec![self.location(false, false)],
            });
        }

        self.write(kind, &wanted)?;

        Ok(PathState {
            locations: vec![self.location(false, true)],
        })
    }

    fn state(&self, dir: &Path) -> Result<PathState> {
        let dir = dir.display().to_string();

        let present = self
            .read()?
            .is_some_and(|(_, value)| entries(&value).any(|entry| same(entry, &dir)));

        Ok(PathState {
            locations: vec![self.location(present, false)],
        })
    }
}

/// An open registry key that closes itself.
///
/// Every early return above would otherwise leak a handle, and the one in [`Env::read`] between the
/// two queries is on the ordinary path rather than an exotic one.
#[derive(Debug)]
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        #[expect(
            unsafe_code,
            reason = "the handle came from RegOpenKeyExW/RegCreateKeyExW in this module and is \
                      closed exactly once, here"
        )]
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// The semicolon-separated entries of a `PATH`, exactly as they are written.
///
/// Nothing is trimmed or dropped here: an empty segment is preserved so that joining the survivors
/// back together reproduces the user's value minus ours and nothing else.
fn entries(value: &str) -> impl Iterator<Item = &str> {
    value.split(';')
}

/// Do two `PATH` entries name the same directory?
///
/// Case-insensitively, because NTFS is; without a trailing separator, because `C:\x` and `C:\x\`
/// are one directory and both spellings are written by hand; and without the quotes Windows allows
/// around an entry containing a space. `%VAR%` is compared literally rather than expanded — what is
/// being looked for is *our* entry, which is always a resolved path.
fn same(entry: &str, dir: &str) -> bool {
    fn normalise(text: &str) -> String {
        text.trim()
            .trim_matches('"')
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    }

    !entry.trim().is_empty() && normalise(entry) == normalise(dir)
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

    /// Where these tests keep their own `Path`, one key per test so they can run in parallel.
    ///
    /// Under `Software` rather than under `Environment`: what is being exercised is the registry
    /// code — the size query, the type round trip, the entry arithmetic — and exercising it against
    /// the value that decides whether the person running `cargo test` can find `git` tomorrow would
    /// be a test that is one bug away from ruining their afternoon.
    struct TestKey(&'static str);

    impl TestKey {
        fn env(&self) -> Env {
            Env { key: self.0 }
        }

        fn set(&self, kind: REG_VALUE_TYPE, value: &str) {
            self.env()
                .write(kind, value)
                .expect("the test key is writable");
        }

        fn get(&self) -> Option<(REG_VALUE_TYPE, String)> {
            self.env().read().expect("the test key is readable")
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            // Through `reg.exe` rather than a fourth registry entry point: this is cleanup, and one
            // more `unsafe` block that only test code reaches would be paying in the product for
            // something no shipped path needs.
            let mut command = std::process::Command::new("reg");
            command.args(["delete", &format!(r"HKCU\{}", self.0), "/f"]);
            super::super::process::without_a_window(&mut command);
            let _ = command.output();
        }
    }

    #[test]
    fn a_user_with_no_path_at_all_gets_one() {
        let key = TestKey(r"Software\MixEngineTest-PathAbsent");
        let env = key.env();

        assert_eq!(key.get(), None, "the test starts with nothing");

        let state = env.add(Path::new(r"C:\MixEngine\bin")).unwrap();
        assert!(state.complete() && state.changed());

        let (kind, value) = key.get().expect("written");
        assert_eq!(value, r"C:\MixEngine\bin");
        assert_eq!(
            kind, REG_EXPAND_SZ,
            "the type Windows creates a user Path as"
        );
    }

    /// The bug that would cost somebody every `%USERPROFILE%`-relative entry they have.
    #[test]
    fn an_expandable_path_is_written_back_expandable() {
        let key = TestKey(r"Software\MixEngineTest-PathType");
        key.set(REG_EXPAND_SZ, r"%USERPROFILE%\go\bin;C:\Tools");

        key.env().add(Path::new(r"C:\MixEngine\bin")).unwrap();

        let (kind, value) = key.get().expect("written");
        assert_eq!(kind, REG_EXPAND_SZ);
        assert_eq!(value, r"C:\MixEngine\bin;%USERPROFILE%\go\bin;C:\Tools");
    }

    /// Everything that is not ours comes back byte for byte, trailing semicolon included.
    #[test]
    fn a_path_comes_back_exactly_as_it_was() {
        let key = TestKey(r"Software\MixEngineTest-PathRoundTrip");
        let original = r"C:\Tools;;%USERPROFILE%\go\bin;";
        key.set(REG_EXPAND_SZ, original);

        let env = key.env();
        env.add(Path::new(r"C:\MixEngine\bin")).unwrap();
        assert_ne!(key.get().unwrap().1, original);

        let removed = env.remove(Path::new(r"C:\MixEngine\bin")).unwrap();
        assert!(removed.changed() && !removed.complete());
        assert_eq!(key.get().unwrap().1, original);
    }

    #[test]
    fn adding_what_is_already_there_changes_nothing() {
        let key = TestKey(r"Software\MixEngineTest-PathIdempotent");
        // Spelled differently on purpose: a trailing separator, a different case and the quotes
        // Windows allows are all the same directory.
        key.set(REG_EXPAND_SZ, r#""c:\mixengine\BIN\";C:\Tools"#);

        let env = key.env();
        let state = env.add(Path::new(r"C:\MixEngine\bin")).unwrap();

        assert!(state.complete() && !state.changed());
        assert_eq!(key.get().unwrap().1, r#""c:\mixengine\BIN\";C:\Tools"#);
        assert!(
            env.state(Path::new(r"C:\MixEngine\bin"))
                .unwrap()
                .complete()
        );
    }

    #[test]
    fn removing_from_a_user_with_no_path_writes_nothing() {
        let key = TestKey(r"Software\MixEngineTest-PathNothingToRemove");
        let env = key.env();

        let state = env.remove(Path::new(r"C:\MixEngine\bin")).unwrap();
        assert!(!state.changed() && !state.complete());
        assert_eq!(key.get(), None, "no value was created to report on");
    }

    /// The whole reason `setx` is not used: a real developer's `Path` is longer than it truncates
    /// at, and losing the second half of one is not recoverable.
    #[test]
    fn a_path_past_what_setx_would_truncate_survives() {
        let key = TestKey(r"Software\MixEngineTest-PathLong");
        let long = (0..80)
            .map(|n| format!(r"C:\Program Files\Something With A Long Name {n}\bin"))
            .collect::<Vec<_>>()
            .join(";");
        assert!(long.len() > 1024, "the fixture has to exceed setx's limit");

        key.set(REG_EXPAND_SZ, &long);
        key.env().add(Path::new(r"C:\MixEngine\bin")).unwrap();

        assert_eq!(key.get().unwrap().1, format!(r"C:\MixEngine\bin;{long}"));
    }
}
