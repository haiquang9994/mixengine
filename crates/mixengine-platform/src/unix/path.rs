//! A tagged block in the shell profiles this user's login shells read.
//!
//! There is no such thing as "the environment" on Unix: a variable a new terminal inherits comes
//! from a file some shell sourced, and which file that is depends on the shell. So this writes one
//! block, between markers, into every profile the OS in question actually reads — `linux/` and
//! `macos/` each name their own list, because the two disagree about which those are and about
//! which one to create when a home has none.
//!
//! # Why a block and not a line
//!
//! `.claude/architecture/platform-abstraction.md`'s first rule: every mutation is reversible and
//! tagged. Nothing outside [`BEGIN`]…[`END`] is ever read, moved or rewritten, which is what makes
//! `remove` able to promise that a profile somebody has been editing since 2011 comes back exactly
//! as it was. The block is also *replaced* rather than added to, so a home that moved leaves one
//! block naming the new directory rather than two naming both.
//!
//! # Why the guard inside it
//!
//! `~/.profile` is read once per login and `~/.bash_profile` once per login shell — but a login
//! shell inside a login shell is an ordinary thing (`bash -l` in a multiplexer, `ssh` into your own
//! machine), and an unguarded `PATH="$dir:$PATH"` grows the variable every time. The `case` is
//! POSIX, understood by `sh`, `dash`, `bash` and `zsh` alike, and the quoting inside its pattern is
//! what makes a directory containing `*` or `[` match itself rather than something else: in a
//! `case` pattern, a quoted stretch is matched literally.
//!
//! **`fish` and `nushell` are not covered**, deliberately rather than by oversight: neither reads a
//! POSIX profile, and a `config.fish` written in fish's own syntax is a second dialect to keep
//! correct for a shell the block would have to be rewritten in. `state` names the files it wrote,
//! so a fish user can see that theirs is not among them.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::{Error, PathIntegration, PathLocation, PathState, Result};

/// The first line of the block. Nothing above it is ours.
const BEGIN: &str = "# BEGIN MixEngine";

/// The last line of the block. Nothing below it is ours.
const END: &str = "# END MixEngine";

/// The shell profiles of one user, and the rules for which of them to write.
#[derive(Debug)]
pub(crate) struct Profiles {
    /// The user's home directory, or why the OS would not say where it is.
    ///
    /// Held as a `Result` shape rather than resolved on every call, and as a failure rather than a
    /// panic: a daemon running as a service account has no home, and the answer to
    /// "put `bin/` on the PATH" there is a sentence rather than a crash.
    home: Option<PathBuf>,

    /// Every file this OS's login shells read, in the order a person would look for them.
    ///
    /// All of the ones that *exist* are written, because which one matters is decided by a login
    /// shell this process is not: a home with both `.bash_profile` and `.zprofile` belongs to
    /// somebody who uses both.
    candidates: &'static [&'static str],

    /// The one to create when a home has none of them.
    ///
    /// Creating a profile is a change to a home that had none, so which file it is matters: it has
    /// to be one the platform's *default* shell reads, or the block lands somewhere nothing looks.
    fallback: &'static str,
}

impl Profiles {
    /// The profiles of the user this process runs as.
    pub(crate) fn of_this_user(
        candidates: &'static [&'static str],
        fallback: &'static str,
    ) -> Self {
        Self {
            home: directories::BaseDirs::new().map(|base| base.home_dir().to_path_buf()),
            candidates,
            fallback,
        }
    }

    /// The same, in a directory a test owns.
    #[cfg(test)]
    pub(crate) fn under(
        home: impl Into<PathBuf>,
        candidates: &'static [&'static str],
        fallback: &'static str,
    ) -> Self {
        Self {
            home: Some(home.into()),
            candidates,
            fallback,
        }
    }

    /// Which files this call is about: the ones that exist, or the fallback when none does.
    ///
    /// Answered afresh on every call rather than cached, because a person who installs `zsh` and
    /// then asks again should get their `.zprofile` written rather than the answer this daemon
    /// worked out at boot.
    fn targets(&self, creating: bool) -> Result<Vec<PathBuf>> {
        let home = self.home.as_ref().ok_or(Error::UnsupportedPlatform {
            capability: "PathIntegration",
            reason: "this account has no home directory, so there is no shell profile to write \
                     — put <root>/bin on PATH yourself, in whatever this environment reads"
                .to_owned(),
        })?;

        let existing: Vec<PathBuf> = self
            .candidates
            .iter()
            .map(|name| home.join(name))
            .filter(|profile| profile.exists())
            .collect();

        // A home with no profile at all gets one **only when something is being added**. Removing
        // from a file that does not exist would otherwise create it in order to report that it does
        // not carry our block, which is a mutation performed by an uninstall.
        if existing.is_empty() && creating {
            return Ok(vec![home.join(self.fallback)]);
        }

        Ok(existing)
    }
}

impl PathIntegration for Profiles {
    fn add(&self, dir: &Path) -> Result<PathState> {
        let block = block(dir);
        let mut locations = Vec::new();

        for profile in self.targets(true)? {
            let existing = read(&profile)?;
            let wanted = with_block(&existing, &block);

            let changed = wanted != existing;
            if changed {
                write(&profile, &wanted)?;
            }

            locations.push(PathLocation {
                name: profile.display().to_string(),
                present: true,
                changed,
            });
        }

        Ok(PathState { locations })
    }

    /// **The directory is not read**, and that is the behaviour rather than an omission: what is
    /// removed is *our block*, whichever directory it happens to name. A home that was moved leaves
    /// a block pointing at the old one, and an uninstall that matched on the current path would
    /// walk past exactly the line nobody else is ever going to delete.
    fn remove(&self, _dir: &Path) -> Result<PathState> {
        let mut locations = Vec::new();

        for profile in self.targets(false)? {
            let existing = read(&profile)?;
            let wanted = without_block(&existing);

            let changed = wanted != existing;
            if changed {
                write(&profile, &wanted)?;
            }

            locations.push(PathLocation {
                name: profile.display().to_string(),
                present: false,
                changed,
            });
        }

        Ok(PathState { locations })
    }

    fn state(&self, dir: &Path) -> Result<PathState> {
        let block = block(dir);
        let mut locations = Vec::new();

        for profile in self.targets(false)? {
            let existing = read(&profile)?;

            locations.push(PathLocation {
                name: profile.display().to_string(),
                // Compared against the block this build would write, not merely against the
                // markers: a block naming the *previous* home is a block that puts a directory
                // which no longer exists on the PATH, and calling that "present" would make
                // `mix path install` a no-op exactly when it is needed.
                present: block_of(&existing).is_some_and(|found| found == block),
                changed: false,
            });
        }

        Ok(PathState { locations })
    }
}

/// The block this build writes, ending in a newline.
fn block(dir: &Path) -> String {
    let quoted = quote(&dir.display().to_string());

    format!(
        "{BEGIN}\n\
         # Added by MixEngine. `mix path uninstall` removes exactly these lines.\n\
         case \":$PATH:\" in\n\
         \x20 *:{quoted}:*) ;;\n\
         \x20 *) PATH={quoted}:$PATH ;;\n\
         esac\n\
         export PATH\n\
         {END}\n"
    )
}

/// One directory, as a shell reads it back as itself.
///
/// Double quotes rather than single, because a Windows-shaped path never reaches here but a home
/// directory with an apostrophe in it does — `/home/o'brien` inside single quotes would end the
/// string. What has to be escaped inside double quotes is exactly four characters, and every other
/// one — spaces, `*`, `[`, `~` — is already literal.
fn quote(dir: &str) -> String {
    let mut quoted = String::with_capacity(dir.len() + 2);
    quoted.push('"');

    for character in dir.chars() {
        if matches!(character, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(character);
    }

    quoted.push('"');
    quoted
}

/// `contents` with our block replaced by `block`, or with it appended when there is none.
fn with_block(contents: &str, block: &str) -> String {
    let mut rest = without_block(contents);

    if !rest.is_empty() && !rest.ends_with('\n') {
        rest.push('\n');
    }

    // One blank line of separation, and only when there is something to separate from. The strip
    // below takes it back, so adding and removing in turn leaves the file it started as.
    if !rest.is_empty() {
        rest.push('\n');
    }

    rest.push_str(block);
    rest
}

/// `contents` with our block — and the blank line [`with_block`] put in front of it — removed.
///
/// Everything outside the markers survives verbatim, including a second block somebody pasted by
/// hand: only the **first** is ours to manage, because a file with two is a file somebody edited
/// and the honest repair is to leave what we did not write.
fn without_block(contents: &str) -> String {
    let Some((start, end)) = span(contents) else {
        return contents.to_owned();
    };

    let mut before = &contents[..start];

    // The separator [`with_block`] adds, and only one of it: a blank line the user left there
    // themselves before we ever wrote is theirs.
    if let Some(trimmed) = before.strip_suffix("\n\n") {
        before = &contents[..trimmed.len() + 1];
    }

    format!("{before}{}", &contents[end..])
}

/// The block itself, when the file has one.
fn block_of(contents: &str) -> Option<&str> {
    let (start, end) = span(contents)?;
    Some(&contents[start..end])
}

/// Byte offsets of the first `# BEGIN MixEngine` line through the end of its `# END MixEngine`
/// line, newline included.
///
/// Lines are matched after trimming, so a block that was indented by an editor is still found — and
/// a `# BEGIN MixEngine` with no `# END` after it is **not** a block: half a marker is either a
/// file that was truncated mid-write or a comment somebody typed, and consuming the rest of a
/// profile because of it is the one failure this whole approach exists to make impossible.
fn span(contents: &str) -> Option<(usize, usize)> {
    let mut start = None;

    for (offset, line) in line_offsets(contents) {
        let line = line.trim();

        if start.is_none() && line == BEGIN {
            start = Some(offset);
        } else if start.is_some() && line == END {
            return start.map(|start| (start, offset + line_len(contents, offset)));
        }
    }

    None
}

/// Every line of `contents` with the offset it starts at.
fn line_offsets(contents: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;

    contents.split_inclusive('\n').map(move |line| {
        let at = offset;
        offset += line.len();
        (at, line.trim_end_matches('\n'))
    })
}

/// How many bytes the line starting at `offset` occupies, its newline included.
fn line_len(contents: &str, offset: usize) -> usize {
    contents[offset..]
        .find('\n')
        .map_or(contents.len() - offset, |end| end + 1)
}

/// A profile's contents, or the empty string when it is not there yet.
fn read(profile: &Path) -> Result<String> {
    match std::fs::read_to_string(profile) {
        Ok(contents) => Ok(contents),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(Error::Io {
            action: "read",
            path: profile.to_path_buf(),
            source,
        }),
    }
}

/// Replace a profile atomically, keeping the permissions it already had.
///
/// A temporary file beside it and a rename, per the platform layer's second rule: a machine that
/// loses power half way through must find either the old profile or the new one, and never a
/// truncated `.zprofile` that stops a user from logging in.
fn write(profile: &Path, contents: &str) -> Result<()> {
    let directory = profile.parent().ok_or_else(|| Error::Io {
        action: "write",
        path: profile.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a shell profile with no directory to write it in",
        ),
    })?;

    // Named after this process, so two MixEngines racing on one home cannot each write half of the
    // other's temporary file. The rename that follows is what serialises them: one of the two
    // profiles wins whole, and neither is ever partial.
    let temporary = directory.join(format!(
        ".{}.mixengine-{}",
        profile
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .trim_start_matches('.'),
        std::process::id()
    ));

    // Every failure below leaves the temporary file behind unless it is removed, and every one of
    // them names a path — so the error is built by one function taking the path it is about rather
    // than by a closure that captures one.
    let failed = |path: &Path, source: std::io::Error| Error::Io {
        action: "write",
        path: path.to_path_buf(),
        source,
    };

    let mut file =
        std::fs::File::create(&temporary).map_err(|source| failed(&temporary, source))?;

    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all());

    if let Err(source) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(&temporary, source));
    }

    // The mode the profile already had, never a fresh one: a `.profile` somebody deliberately made
    // `0600` must not come back `0644` because MixEngine rewrote it. A file that does not exist yet
    // keeps whatever the umask gave the temporary.
    if let Ok(metadata) = std::fs::metadata(profile) {
        let _ = std::fs::set_permissions(&temporary, metadata.permissions());
    }

    if let Err(source) = std::fs::rename(&temporary, profile) {
        let _ = std::fs::remove_file(&temporary);
        return Err(failed(profile, source));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What both real hosts pass, in the shape they pass it.
    const CANDIDATES: &[&str] = &[".profile", ".bash_profile", ".zprofile"];
    const FALLBACK: &str = ".profile";

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary home")
    }

    fn profiles(home: &tempfile::TempDir) -> Profiles {
        Profiles::under(home.path(), CANDIDATES, FALLBACK)
    }

    fn contents(home: &tempfile::TempDir, name: &str) -> String {
        std::fs::read_to_string(home.path().join(name)).expect("the profile was written")
    }

    #[test]
    fn a_home_with_no_profile_at_all_gets_the_one_its_platform_reads() {
        let home = home();
        let state = profiles(&home)
            .add(Path::new("/opt/mixengine/bin"))
            .unwrap();

        assert!(state.complete() && state.changed());
        assert_eq!(state.locations.len(), 1);
        assert!(contents(&home, FALLBACK).contains("/opt/mixengine/bin"));
    }

    /// The whole point of writing every profile that exists: which one a login shell reads is
    /// decided by a shell this process is not.
    #[test]
    fn every_profile_that_exists_is_written_and_none_that_does_not_is_created() {
        let home = home();
        std::fs::write(home.path().join(".bash_profile"), "export EDITOR=vi\n").unwrap();
        std::fs::write(home.path().join(".zprofile"), "").unwrap();

        let state = profiles(&home)
            .add(Path::new("/opt/mixengine/bin"))
            .unwrap();

        assert_eq!(state.locations.len(), 2, "{state:?}");
        assert!(state.complete());
        assert!(!home.path().join(".profile").exists(), "not created");
        assert!(contents(&home, ".bash_profile").starts_with("export EDITOR=vi\n"));
    }

    /// A login shell inside a login shell must not double the entry — see the module note.
    #[test]
    fn the_block_only_prepends_when_the_directory_is_not_already_there() {
        let block = block(Path::new("/opt/mixengine/bin"));

        assert!(block.contains(r#"case ":$PATH:" in"#), "{block}");
        assert!(block.contains(r#"*:"/opt/mixengine/bin":*) ;;"#), "{block}");
    }

    /// Adding twice writes once. A caller that reported an install it did not perform would be
    /// indistinguishable from one that did.
    #[test]
    fn adding_what_is_already_there_changes_nothing() {
        let home = home();
        let profiles = profiles(&home);
        let dir = Path::new("/opt/mixengine/bin");

        profiles.add(dir).unwrap();
        let before = contents(&home, FALLBACK);

        let again = profiles.add(dir).unwrap();
        assert!(again.complete() && !again.changed());
        assert_eq!(contents(&home, FALLBACK), before);

        let state = profiles.state(dir).unwrap();
        assert!(state.complete() && !state.changed());
    }

    /// The promise the whole marked-block approach exists to make.
    #[test]
    fn a_profile_comes_back_exactly_as_it_was() {
        let home = home();
        let profiles = profiles(&home);
        let original = "# mine\nexport EDITOR=vi\n\nalias ll='ls -l'\n";
        std::fs::write(home.path().join(".profile"), original).unwrap();

        profiles.add(Path::new("/opt/mixengine/bin")).unwrap();
        assert_ne!(contents(&home, ".profile"), original);

        let removed = profiles.remove(Path::new("/opt/mixengine/bin")).unwrap();
        assert!(removed.changed() && !removed.complete());
        assert_eq!(contents(&home, ".profile"), original);
    }

    /// A home that moved leaves one block naming the new directory, never two naming both.
    #[test]
    fn a_second_home_replaces_the_block_rather_than_adding_one() {
        let home = home();
        let profiles = profiles(&home);

        profiles.add(Path::new("/opt/old/bin")).unwrap();
        profiles.add(Path::new("/opt/new/bin")).unwrap();

        let written = contents(&home, FALLBACK);
        assert_eq!(written.matches(BEGIN).count(), 1, "{written}");
        assert!(!written.contains("/opt/old/bin"), "{written}");

        // And the stale one is not mistaken for ours being in place.
        assert!(
            !profiles
                .state(Path::new("/opt/old/bin"))
                .unwrap()
                .complete()
        );
    }

    /// Half a marker is a comment somebody typed or a file that was truncated mid-write. Eating the
    /// rest of a profile because of it is the failure this approach exists to make impossible.
    #[test]
    fn an_unterminated_marker_is_not_a_block() {
        let opened = format!("export EDITOR=vi\n{BEGIN}\nPATH=/somewhere:$PATH\n");

        assert_eq!(span(&opened), None);
        assert_eq!(without_block(&opened), opened);
    }

    /// The quoting is what lets a path with a glob character in it match itself.
    #[test]
    fn a_directory_a_shell_would_otherwise_expand_is_written_literally() {
        assert_eq!(quote("/home/o'brien/[dev]"), r#""/home/o'brien/[dev]""#);
        assert_eq!(quote(r#"/home/a "b"/$PATH"#), r#""/home/a \"b\"/\$PATH""#);
    }

    /// An uninstall must not create the file it is reporting on.
    #[test]
    fn removing_from_a_home_with_no_profile_creates_nothing() {
        let home = home();
        let state = profiles(&home)
            .remove(Path::new("/opt/mixengine/bin"))
            .unwrap();

        assert!(state.locations.is_empty() && !state.changed());
        assert!(!home.path().join(FALLBACK).exists());
    }
}
