//! A real archive, in each of the three shapes the publishing pipeline produces.
//!
//! `.claude/standards/testing.md` named this before it existed and said what it is for: *a tiny
//! tarball/zip with a known SHA-256, for install flows without the network*. What it must not be is
//! a stand-in for unpacking — the install pipeline's most interesting steps are the checksum, the
//! entry-path check and the mode bits, and every one of them is a property of a genuine archive.
//!
//! # It is written with different implementations than it is read with
//!
//! Deliberately, and on the same principle as [`MockRegistry`](crate::MockRegistry) signing with
//! `minisign` while the product verifies with `minisign-verify`: `.tar.zst` is compressed here by
//! the reference C library through `zstd` and decompressed in the product by the pure-Rust
//! `ruzstd`. A fixture built with the implementation it is then read by proves only that the
//! implementation agrees with itself.
//!
//! # The hash is computed here, and stated to the test
//!
//! [`Packed::sha256`] is what the caller puts in the index it serves. Asking the code under test for
//! it would make the checksum step assert nothing, which is the same trap
//! [`Home`](crate::Home) avoids by restating the paths it needs rather than computing them.

use std::io::{Cursor, Write as _};
use std::path::PathBuf;

use sha2::Digest as _;

use crate::service::FakeService;

/// Which of the three shapes the publishing pipeline produces.
///
/// The same set `tools/mkindex.py` recognises: Windows artifacts are `.zip`, macOS and Linux ones
/// are `.tar.zst`, and `.tar.gz` is what the pipeline falls back to when the runner's `tar` has no
/// zstd — so all three are published and a test that covered one would cover the platform it
/// happened to run on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Packing {
    /// `.zip`, deflated, as `php_windows.py` packs it.
    Zip,
    /// `.tar.gz`.
    TarGz,
    /// `.tar.zst`.
    TarZst,
}

impl Packing {
    /// Every shape, for a test that should not care which platform it is running on.
    pub const ALL: [Self; 3] = [Self::Zip, Self::TarGz, Self::TarZst];

    /// The file-name suffix, which is also how the product decides what to unpack it with.
    #[must_use]
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Zip => ".zip",
            Self::TarGz => ".tar.gz",
            Self::TarZst => ".tar.zst",
        }
    }
}

/// One entry to be packed.
#[derive(Debug)]
struct Entry {
    name: String,
    contents: Vec<u8>,
    mode: u32,
}

/// An archive under construction.
#[derive(Debug)]
pub struct FakePackage {
    packing: Packing,
    entries: Vec<Entry>,
}

/// An archive, and the two things an index entry has to say about one.
#[derive(Debug, Clone)]
pub struct Packed {
    /// The archive itself, ready to be served.
    pub bytes: Vec<u8>,

    /// Its SHA-256 as lowercase hex — what goes in the index, and what the installer checks against.
    pub sha256: String,

    /// A file name carrying the right suffix, so a URL built from it names the format.
    pub file_name: String,
}

impl FakePackage {
    /// An empty archive of the given shape.
    #[must_use]
    pub fn new(packing: Packing) -> Self {
        Self {
            packing,
            entries: Vec::new(),
        }
    }

    /// Add an ordinary file.
    #[must_use]
    pub fn file(mut self, name: &str, contents: &[u8]) -> Self {
        self.entries.push(Entry {
            name: name.to_owned(),
            contents: contents.to_vec(),
            mode: 0o644,
        });
        self
    }

    /// Add a file that can actually be executed, by copying the `fakeservice` binary in.
    ///
    /// **A real program and not a script**, because the post-install check spawns what it finds:
    /// Windows cannot `CreateProcess` a `.bat` without a shell in front of it, so a fixture built
    /// out of shell scripts would test the check on two platforms and skip it on the third. This is
    /// the same binary the supervisor is tested against, and `--help` is a thing every `clap`
    /// program answers zero to.
    ///
    /// # Panics
    ///
    /// If `fakeservice` cannot be found or read — see [`FakeService::program`], which says what to
    /// run when it cannot.
    #[must_use]
    pub fn executable(mut self, name: &str) -> Self {
        let program = FakeService::program();
        let contents = std::fs::read(&program)
            .unwrap_or_else(|error| panic!("read {}: {error}", program.display()));

        self.entries.push(Entry {
            name: name.to_owned(),
            contents,
            mode: 0o755,
        });
        self
    }

    /// Add an entry under a name of the caller's choosing, however malformed.
    ///
    /// The one method here that exists for a single test: an archive whose entry names somewhere
    /// else (`../escape`) is what an installer's path check is for, and it cannot be built through
    /// [`file`](Self::file) without that method deciding to be lenient about names in general.
    #[must_use]
    pub fn raw_entry(mut self, name: &str, contents: &[u8]) -> Self {
        self.entries.push(Entry {
            name: name.to_owned(),
            contents: contents.to_vec(),
            mode: 0o644,
        });
        self
    }

    /// Pack it, and say what it hashes to.
    ///
    /// # Panics
    ///
    /// If the archive cannot be written, which means the fixture is broken rather than the code
    /// under test.
    #[must_use]
    pub fn build(&self, stem: &str) -> Packed {
        let bytes = match self.packing {
            Packing::Zip => self.zip(),
            Packing::TarGz => {
                let mut encoder =
                    flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
                encoder.write_all(&self.tar()).expect("gzip the tar");
                encoder.finish().expect("finish the gzip stream")
            }
            Packing::TarZst => zstd::stream::encode_all(Cursor::new(self.tar()), 3)
                .expect("zstandard-compress the tar"),
        };

        let sha256 = sha2::Sha256::digest(&bytes)
            .iter()
            .fold(String::new(), |mut hex, byte| {
                use std::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
                hex
            });

        Packed {
            file_name: format!("{stem}{}", self.packing.suffix()),
            bytes,
            sha256,
        }
    }

    /// The zip half, deflated the way `php_windows.py` packs one.
    fn zip(&self) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

        for entry in &self.entries {
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(entry.mode);
            // `start_file` rather than `start_file_from_path`, which normalises a name and would
            // quietly repair the one entry `raw_entry` exists to produce.
            writer
                .start_file(&entry.name, options)
                .expect("start a zip entry");
            writer
                .write_all(&entry.contents)
                .expect("write a zip entry");
        }

        writer.finish().expect("finish the zip").into_inner()
    }

    /// The tar inside both `.tar.gz` and `.tar.zst`.
    fn tar(&self) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());

        for entry in &self.entries {
            let name = entry.name.as_bytes();
            assert!(
                name.len() < 100,
                "a fixture entry name has to fit the old tar header: {}",
                entry.name
            );

            let mut header = tar::Header::new_gnu();
            header.set_size(entry.contents.len() as u64);
            header.set_mode(entry.mode);

            // **The name is written into the header rather than through `set_path`**, which refuses
            // a `..` outright — a good rule for a program writing an archive, and the exact thing
            // `raw_entry` has to be able to do. A tar that cannot be built with a traversal in it
            // is a tar the installer's refusal cannot be tested against.
            if let Some(gnu) = header.as_gnu_mut() {
                gnu.name[..name.len()].copy_from_slice(name);
            }
            // After the name, or it covers a header that is no longer there.
            header.set_cksum();

            builder
                .append(&header, entry.contents.as_slice())
                .expect("write a tar entry");
        }

        builder.into_inner().expect("finish the tar")
    }
}

impl Packed {
    /// Where this would be served from, under `base` — a URL whose suffix names the format.
    #[must_use]
    pub fn path(&self) -> String {
        format!("/{}", self.file_name)
    }

    /// How large it is, which is the `size` an index entry carries.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.bytes.len() as u64
    }
}

/// The `fakeservice` binary's own path, for a test that wants to compare what it installed against
/// what it packed.
#[must_use]
pub fn executable_source() -> PathBuf {
    FakeService::program()
}
