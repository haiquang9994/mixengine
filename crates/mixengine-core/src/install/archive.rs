//! Unpacking a downloaded artifact, without letting it choose where its bytes land.
//!
//! # The entry path is checked here, not only by the crate that writes it
//!
//! Both `tar` and `zip` already refuse to write outside the directory they are handed —
//! `Entry::unpack_in` returns `false` and `ZipFile::enclosed_name` returns [`None`]. This module
//! checks the path itself anyway, *before* either is asked to write, for two reasons that are not
//! the same reason. The refusal becomes ours, so it has a message naming the entry and a test that
//! holds it; and the check stops being a property of whichever crate version is resolved this week,
//! for what is the single most attacked step in installing anything.
//!
//! What is deliberately **not** re-implemented here is everything below the path: the mode bits, the
//! symlinks, the hardlinks. Those are the operating system's business, `.claude/standards/rust.md`
//! says a `#[cfg]` outside `mixengine-platform` fails review, and a hand-rolled unpacker would need
//! one per entry type. Delegating that to the crate whose job it is *is* the platform abstraction
//! here.
//!
//! # Why the format comes from the URL
//!
//! The URL is a field of a document this build has already verified an Ed25519 signature over, so
//! its suffix is as trustworthy as the SHA-256 beside it — and it is the same key
//! `tools/mkindex.py` writes the index from. Sniffing magic bytes instead would read a claim made by
//! the downloaded file about itself, which is a strictly weaker statement than one we signed.

use std::io::{BufReader, Read};
use std::path::{Component, Path};

use crate::{Error, Result};

/// One of the three shapes the publishing pipeline produces.
///
/// A closed set, and it matches `ARCHIVE_SUFFIXES` in that pipeline's `mkindex.py`: an index naming
/// a fourth is describing an artifact this build has no decompressor for, which is a refusal with a
/// reason rather than something to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// `.zip` — what Windows artifacts are packed as.
    Zip,
    /// `.tar.gz` — the fallback the pipeline packs when the runner's `tar` has no zstd.
    TarGz,
    /// `.tar.zst` — what macOS and Linux artifacts are packed as.
    TarZst,
}

impl Format {
    /// Read the format off a URL, or [`None`] for one this build cannot unpack.
    ///
    /// Longest suffix first, because `.tar.gz` also ends with `.gz` and a shorter match would pick
    /// the wrong container for the right compressor.
    pub(crate) fn of(url: &str) -> Option<Self> {
        // A query string or fragment is not part of the file name. Neither appears in a published
        // URL today; a mirror that adds one should still install rather than fail to classify.
        let path = url.split(['?', '#']).next().unwrap_or(url);

        if path.ends_with(".tar.zst") {
            Some(Self::TarZst)
        } else if path.ends_with(".tar.gz") {
            Some(Self::TarGz)
        } else if path.ends_with(".zip") {
            Some(Self::Zip)
        } else {
            None
        }
    }
}

/// Unpack `archive` into `into`, which must already exist and should be empty.
///
/// **Blocking on purpose.** Every byte of an eighty-megabyte archive is decompressed and written by
/// this call, so it belongs on a blocking thread; the caller in [`super`] is what puts it there, on
/// the rule in `.claude/standards/rust.md` that nothing blocks the runtime.
///
/// # Errors
///
/// [`Error::UnsafeArchiveEntry`] when an entry names a path outside `into`,
/// [`Error::ArchiveUnreadable`] when the container or the compressor refuses what it was given, and
/// [`Error::Io`] when the file cannot be opened.
pub(crate) fn extract(archive: &Path, format: Format, into: &Path) -> Result<()> {
    let file = std::fs::File::open(archive).map_err(|source| Error::Io {
        action: "open",
        path: archive.to_path_buf(),
        source,
    })?;
    // Every one of the three reads the file in one forward pass, so a buffer in front of it is the
    // difference between one syscall per block and one per `read` the decompressor asks for.
    let file = BufReader::new(file);

    match format {
        Format::Zip => unpack_zip(file, archive, into),
        Format::TarGz => unpack_tar(flate2::read::GzDecoder::new(file), archive, into),
        Format::TarZst => {
            let stream = ruzstd::decoding::StreamingDecoder::new(file)
                .map_err(|source| unreadable(archive, source))?;
            unpack_tar(stream, archive, into)
        }
    }
}

/// Whether an entry may be written at all.
///
/// Accepts a path made only of ordinary names and `.` — which every `tar -C tree .` entry starts
/// with — and refuses everything that could mean somewhere else: `..`, a leading `/`, and a Windows
/// prefix such as `C:` or `\\?\`. An empty path is refused too; a nameless entry has nowhere to go.
pub(super) fn safe(path: &Path) -> bool {
    let mut named = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => named = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    named
}

/// Unpack a zip, refusing any entry that names somewhere else first.
///
/// The names are walked in full before a single byte is written, so an archive whose last entry is
/// the malicious one does not get to leave the first hundred behind. `by_index_raw` reads the entry
/// header without starting its decompressor, which is what makes that first pass cheap.
fn unpack_zip<R: Read + std::io::Seek>(file: R, archive: &Path, into: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(file).map_err(|source| unreadable(archive, source))?;

    for index in 0..zip.len() {
        let entry = zip
            .by_index_raw(index)
            .map_err(|source| unreadable(archive, source))?;

        // `enclosed_name` is `None` for an absolute path, a `..`, and — the one a reader forgets —
        // a name whose backslashes make it a different path on Windows than on Unix.
        let refused = match entry.enclosed_name() {
            Some(path) => !safe(&path),
            None => true,
        };
        if refused {
            return Err(Error::UnsafeArchiveEntry {
                archive: archive.to_path_buf(),
                entry: entry.name().to_owned(),
            });
        }
    }

    zip.extract(into)
        .map_err(|source| unreadable(archive, source))
}

/// Unpack a tar stream, one entry at a time.
///
/// Streamed rather than walked twice: a `.tar.zst` cannot be rewound without decompressing it
/// again, so the two-pass shape [`unpack_zip`] uses would cost a second full decompression. The trade is
/// that a refusal here can leave earlier entries on disk — which costs nothing, because the caller
/// removes the staging directory on any failure and nothing has been renamed into place yet.
///
/// `preserve_permissions` is set because it is **off by default** in `tar`, and a PHP unpacked
/// without its mode bits is a `php` that cannot be executed — a failure that would surface later,
/// somewhere else, as a permission error nobody would trace back to here.
fn unpack_tar<R: Read>(reader: R, archive: &Path, into: &Path) -> Result<()> {
    let mut tar = tar::Archive::new(reader);
    tar.set_preserve_permissions(true);
    tar.set_overwrite(true);
    // Extended attributes are the publisher's machine leaking into the user's — quarantine flags,
    // SELinux labels — and none of them is something this project means to carry.
    tar.set_unpack_xattrs(false);

    for entry in tar
        .entries()
        .map_err(|source| unreadable(archive, source))?
    {
        let mut entry = entry.map_err(|source| unreadable(archive, source))?;

        let path = entry
            .path()
            .map_err(|source| unreadable(archive, source))?
            .into_owned();
        let refused = |named: &Path| Error::UnsafeArchiveEntry {
            archive: archive.to_path_buf(),
            entry: named.display().to_string(),
        };

        if !safe(&path) {
            return Err(refused(&path));
        }

        // Asked as well as checked. `unpack_in` answers `false` for what it will not write, and the
        // cases it catches and the check above does not are the ones a symlink makes: an entry
        // whose *parent* is a link somebody planted earlier in the same archive.
        if !entry
            .unpack_in(into)
            .map_err(|source| unreadable(archive, source))?
        {
            return Err(refused(&path));
        }
    }

    Ok(())
}

/// Blame the archive rather than the reader, for anything a decompressor refuses.
fn unreadable(
    archive: &Path,
    source: impl std::error::Error + Send + Sync + 'static,
) -> crate::Error {
    Error::ArchiveUnreadable {
        archive: archive.to_path_buf(),
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_three_published_shapes_are_recognised_and_nothing_else_is() {
        let format = |name: &str| Format::of(&format!("https://example.invalid/php-8.3.33{name}"));

        assert_eq!(format(".zip"), Some(Format::Zip));
        assert_eq!(format(".tar.gz"), Some(Format::TarGz));
        assert_eq!(format(".tar.zst"), Some(Format::TarZst));

        // `.tar.gz` ends with `.gz`, and a bare `.gz` is not a container this can unpack.
        assert_eq!(format(".gz"), None);
        assert_eq!(format(".tar"), None);
        assert_eq!(format(".7z"), None);
        assert_eq!(format(""), None);
    }

    #[test]
    fn a_query_string_does_not_hide_the_format() {
        assert_eq!(
            Format::of("https://mirror.invalid/php.tar.zst?token=abc#part"),
            Some(Format::TarZst)
        );
    }

    #[test]
    fn an_entry_that_names_somewhere_else_is_refused() {
        for name in [
            "../outside",
            "a/../../outside",
            "/etc/passwd",
            "",
            ".",
            #[cfg(windows)]
            r"C:\windows\system32",
            #[cfg(windows)]
            r"..\outside",
        ] {
            assert!(!safe(&PathBuf::from(name)), "{name:?} should be refused");
        }
    }

    #[test]
    fn an_ordinary_entry_is_allowed_including_the_leading_dot_tar_writes() {
        for name in ["php.exe", "bin/php", "./bin/php", "ext/php_curl.dll"] {
            assert!(safe(&PathBuf::from(name)), "{name:?} should be allowed");
        }
    }
}
