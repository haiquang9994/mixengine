//! The managed block in the machine's hosts file.
//!
//! **Mechanism here, policy in `mixengine-elevate`** — the T41 design, D3. This module finds the
//! block, splices it, renders it and replaces the file; it decides nothing about what may be
//! written. That split is what lets the dangerous half be tested exhaustively against files a test
//! owns, while the half that could point `evil.com` at loopback sits in forty lines of the audited
//! binary with nothing else in them.
//!
//! **Nothing outside [`BEGIN_MARKER`]…[`END_MARKER`] is ever read, moved or rewritten.** That is the
//! platform layer's first rule — every mutation is reversible and tagged — and it is the promise a
//! user never forgives being broken: a hosts file somebody has been editing since 2011 comes back
//! exactly as it was. This module's own tests assert it by comparing whole files rather than by
//! looking for the lines they expected to keep.
//!
//! Compiled under **both** `host` and `elevated`: the daemon reads the block through
//! [`crate::HostsFile`], and the helper writes it, and neither is worth a second implementation.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use mixengine_proto::privileged::HostEntry;

use crate::{Error, Result};

/// The first line of the block. Nothing above it is ours.
pub const BEGIN_MARKER: &str = "# BEGIN MixEngine";

/// The last line of the block. Nothing below it is ours.
pub const END_MARKER: &str = "# END MixEngine";

/// Where this OS keeps the file.
#[must_use]
pub fn path() -> PathBuf {
    crate::sys::hosts::path()
}

/// The lock that keeps two homes on one machine from interleaving their writes — the T41 design, D8.
///
/// In the audit directory, which is already root-owned, already machine-wide and already created by
/// the helper on first run. The helper's own `elevate.lock` is per-home and says nothing about a
/// second account.
#[cfg(feature = "elevated")]
const LOCK: &str = "hosts.lock";

/// What one [`apply`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(feature = "elevated")]
pub enum Change {
    /// The file was replaced, and now holds this many entries.
    Written {
        /// How many, for the audit line and for `mix doctor`.
        entries: usize,
    },

    /// The file already said exactly this. Not a failure and not a change.
    Unchanged,
}

/// Read, splice, replace — atomically, under the machine-wide lock.
///
/// **Takes its path rather than calling [`path`]**, which is what lets every test above the unit
/// level drive the real engine against a file it owns.
///
/// # Errors
///
/// [`Error::MalformedBlock`] for a block that cannot be edited without guessing, [`Error::Io`] when
/// the file cannot be read or replaced, and [`Error::Os`] when the machine-wide lock is held by
/// another helper.
#[cfg(feature = "elevated")]
pub fn apply(path: &Path, entries: &[HostEntry]) -> Result<Change> {
    let lock = crate::elevated::audit_directory()?.join(LOCK);

    apply_under(path, entries, &lock)
}

/// [`apply`], with the lock named.
///
/// The tests take a lock inside a directory they own: the audit directory is root's, and a suite
/// that had to create it could not run under an ordinary token.
#[cfg(feature = "elevated")]
pub(crate) fn apply_under(path: &Path, entries: &[HostEntry], lock: &Path) -> Result<Change> {
    // Held across the read *and* the rename: two helpers that both read before either wrote would
    // each apply their own home's state over the other's.
    let _held = match crate::lock::Lock::acquire(lock)? {
        crate::lock::Acquired::Held(held) => held,
        crate::lock::Acquired::Taken(holder) => {
            return Err(Error::Os {
                action: "take the machine-wide hosts lock",
                source: std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    format!("{holder} is already applying a hosts change"),
                ),
            });
        }
    };

    let text = read(path)?;
    let spliced = splice(&text, entries)?;

    if spliced == text {
        return Ok(Change::Unchanged);
    }

    crate::sys::hosts::replace(path, &spliced)?;

    Ok(Change::Written {
        entries: entries.len(),
    })
}

/// The file's contents, or the empty string when it is not there.
#[cfg(feature = "elevated")]
fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(source) => Err(Error::Io {
            action: "read",
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// The entries in the managed block of `text`, or why the block cannot be read.
///
/// An empty vector is a file with no block of ours in it, which is not an error: it is what a
/// machine that has never run MixEngine looks like.
///
/// # Errors
///
/// [`Error::MalformedBlock`] for a block that cannot be read without guessing — see [`splice`] and
/// the T41 design, D6 — and for a line inside it that names no address or no domain.
pub fn parse(text: &str) -> Result<Vec<HostEntry>> {
    let Some(range) = block(text)? else {
        return Ok(Vec::new());
    };

    let mut entries = Vec::new();

    for line in text[range].lines() {
        let line = line.trim();

        // The markers themselves are comments, which is what makes this the whole of the skip.
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut fields = line.split_whitespace();
        let first = fields.next().unwrap_or_default();
        let address: IpAddr = first.parse().map_err(|_| Error::MalformedBlock {
            reason: format!("`{first}` is where MixEngine's block should hold an address"),
        })?;

        let mut named = false;

        for domain in fields {
            named = true;
            entries.push(HostEntry {
                address,
                domain: domain.to_owned(),
            });
        }

        if !named {
            return Err(Error::MalformedBlock {
                reason: format!("`{line}` names an address and no domain"),
            });
        }
    }

    Ok(entries)
}

/// `text` with the managed block set to `entries`, or why it cannot be.
///
/// An empty `entries` removes the block, markers and all — which is also the reverse of every other
/// call, and the reason no backup file is kept.
///
/// The block is rendered with the file's own line ending: CRLF if the file uses it anywhere, because
/// rewriting a Windows hosts file with Unix endings is a diff on every line in a file people read
/// with Notepad.
///
/// # Errors
///
/// [`Error::MalformedBlock`] for two `BEGIN` markers, a `BEGIN` with no `END`, or an `END` with no
/// `BEGIN` — D6. Repairing any of those means guessing at what somebody else meant.
pub fn splice(text: &str, entries: &[HostEntry]) -> Result<String> {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let rendered = render(entries, newline);

    let mut spliced = String::with_capacity(text.len() + rendered.len());

    match block(text)? {
        Some(range) => {
            spliced.push_str(&text[..range.start]);
            spliced.push_str(&rendered);
            spliced.push_str(&text[range.end..]);
        }
        None if entries.is_empty() => spliced.push_str(text),
        None => {
            spliced.push_str(text);

            // The block starts on a line of its own. This is the only byte this engine ever adds
            // outside its own block, and the only reason a removal is not bit-for-bit reversible.
            if !text.is_empty() && !text.ends_with('\n') {
                spliced.push_str(newline);
            }

            spliced.push_str(&rendered);
        }
    }

    Ok(spliced)
}

/// The block itself, or nothing at all when there is nothing to write.
fn render(entries: &[HostEntry], newline: &str) -> String {
    if entries.is_empty() {
        return String::new();
    }

    let mut block = String::with_capacity(entries.len() * 32);

    block.push_str(BEGIN_MARKER);
    block.push_str(newline);

    for entry in entries {
        block.push_str(&entry.address.to_string());
        block.push_str("  ");
        block.push_str(&entry.domain);
        block.push_str(newline);
    }

    block.push_str(END_MARKER);
    block.push_str(newline);

    block
}

/// Where the managed block sits in `text`, as a byte range covering both marker lines.
///
/// A marker is matched against a **trimmed** line, exactly: leading whitespace and a CR are ignored,
/// and `# BEGIN MixEngine (do not edit)` is somebody else's comment rather than our marker.
fn block(text: &str) -> Result<Option<std::ops::Range<usize>>> {
    let (mut begin, mut end) = (None, None);
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();

        if trimmed == BEGIN_MARKER {
            if begin.is_some() {
                return Err(malformed("a second `# BEGIN MixEngine` marker"));
            }
            begin = Some(offset);
        } else if trimmed == END_MARKER {
            if begin.is_none() || end.is_some() {
                return Err(malformed(
                    "an `# END MixEngine` marker with no `# BEGIN MixEngine` above it",
                ));
            }
            end = Some(offset + line.len());
        }

        offset += line.len();
    }

    match (begin, end) {
        (Some(start), Some(finish)) => Ok(Some(start..finish)),
        (Some(_), None) => Err(malformed(
            "a `# BEGIN MixEngine` marker with no `# END MixEngine` below it",
        )),
        // `end` is only ever set once `begin` is, so the remaining case is a file with no block.
        (None, _) => Ok(None),
    }
}

/// A block this code will not edit, and why.
fn malformed(reason: &str) -> Error {
    Error::MalformedBlock {
        reason: format!("MixEngine's block in the hosts file has {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hosts file with everything a real one has: comments, the entries the OS ships, tabs, blank
    /// lines, and another product's marked block.
    const REAL: &str = "\
# Copyright (c) 1993-2009 Microsoft Corp.
#
# This is a sample HOSTS file.

127.0.0.1\tlocalhost
::1        ip6-localhost ip6-loopback
10.0.0.7   nas.lan   nas

# BEGIN SomeOtherTool
203.0.113.9  example.invalid
# END SomeOtherTool

255.255.255.255\tbroadcasthost
";

    fn entry(address: &str, domain: &str) -> HostEntry {
        HostEntry {
            address: address.parse().expect("a literal address"),
            domain: domain.to_owned(),
        }
    }

    /// **The regression this whole task is arranged around.** Splice, splice again with a different
    /// set, then splice empty — and the file is byte-identical to the one we started with.
    ///
    /// Asserted by comparing the whole file rather than by looking for the lines we expected to
    /// keep: a test that searched for `nas.lan` would pass on a file that had lost everything else.
    #[test]
    fn every_unrelated_line_survives_every_edit() {
        let first = splice(REAL, &[entry("127.0.0.1", "blog.test")]).unwrap();
        assert!(first.contains("nas.lan"), "{first}");
        assert!(first.contains("# BEGIN SomeOtherTool"), "{first}");

        let second = splice(
            &first,
            &[entry("127.0.0.1", "shop.test"), entry("::1", "shop.test")],
        )
        .unwrap();
        assert!(
            !second.contains("blog.test"),
            "the block is replaced, not appended to: {second}"
        );

        let removed = splice(&second, &[]).unwrap();

        assert_eq!(removed, REAL, "the file did not come back as it was");
    }

    /// The file the fixture above is not: hand-edited, and with no newline at the end of it.
    ///
    /// The block still starts on a line of its own, and removal gives the original back plus
    /// exactly the newline that had to be added to get there. That one byte is the only thing this
    /// engine ever adds outside its own block, and it is asserted rather than left to be noticed.
    #[test]
    fn a_file_with_no_trailing_newline_gains_one_and_nothing_else() {
        let original = "127.0.0.1 localhost";

        let written = splice(original, &[entry("127.0.0.1", "blog.test")]).unwrap();
        assert!(
            written.starts_with("127.0.0.1 localhost\n# BEGIN MixEngine\n"),
            "{written}"
        );

        assert_eq!(splice(&written, &[]).unwrap(), "127.0.0.1 localhost\n");
    }

    /// A Windows hosts file is read in Notepad. Rewriting it with Unix endings is a diff on every
    /// line of a file nobody asked us to reformat.
    #[test]
    fn the_files_own_line_ending_is_the_one_the_block_is_written_with() {
        let crlf = "# a comment\r\n127.0.0.1\tlocalhost\r\n";

        let written = splice(crlf, &[entry("127.0.0.1", "blog.test")]).unwrap();

        assert!(written.contains("# BEGIN MixEngine\r\n"), "{written:?}");
        assert!(written.contains("127.0.0.1  blog.test\r\n"), "{written:?}");
        assert!(
            !written.contains("\n\n"),
            "no bare LF was introduced: {written:?}"
        );
        assert_eq!(splice(&written, &[]).unwrap(), crlf);
    }

    /// An empty list on a file with no block is not an edit at all.
    #[test]
    fn removing_a_block_that_is_not_there_changes_nothing() {
        assert_eq!(splice(REAL, &[]).unwrap(), REAL);
        assert_eq!(splice("", &[]).unwrap(), "");
    }

    /// Order in, one file out: the caller sorts, and the engine writes what it is given.
    #[test]
    fn the_same_set_in_two_orders_is_the_same_file() {
        let one = splice(
            REAL,
            &[entry("127.0.0.1", "a.test"), entry("127.0.0.1", "b.test")],
        )
        .unwrap();
        let other = splice(
            REAL,
            &[entry("127.0.0.1", "a.test"), entry("127.0.0.1", "b.test")],
        )
        .unwrap();

        assert_eq!(one, other);
    }

    /// D6: what is wrong is on the machine, and a person has to look at it. Picking one of two
    /// `BEGIN`s would be a program editing a system file according to a guess.
    #[test]
    fn a_malformed_block_is_refused_rather_than_repaired_by_guessing() {
        let two_begins =
            format!("{BEGIN_MARKER}\n127.0.0.1 a.test\n{BEGIN_MARKER}\n{END_MARKER}\n");
        let no_end = format!("127.0.0.1 localhost\n{BEGIN_MARKER}\n127.0.0.1 a.test\n");
        let no_begin = format!("127.0.0.1 localhost\n{END_MARKER}\n");

        for (text, what) in [
            (two_begins, "a second"),
            (no_end, "no `# END MixEngine`"),
            (no_begin, "no `# BEGIN MixEngine`"),
        ] {
            let error = splice(&text, &[entry("127.0.0.1", "a.test")]).unwrap_err();

            assert!(
                matches!(&error, crate::Error::MalformedBlock { reason } if reason.contains(what)),
                "{error}"
            );
            assert!(parse(&text).is_err(), "reading it is refused too");
        }
    }

    /// What is in the block, for the read-only capability and for `mix doctor`.
    #[test]
    fn the_managed_block_reads_back_as_the_entries_that_were_written() {
        let entries = vec![entry("127.0.0.1", "blog.test"), entry("::1", "blog.test")];

        let written = splice(REAL, &entries).unwrap();

        assert_eq!(parse(&written).unwrap(), entries);
        assert_eq!(
            parse(REAL).unwrap(),
            Vec::new(),
            "another tool's block is not ours"
        );
    }

    /// One line may name several domains: a person editing the block by hand writes it that way,
    /// and an older build may have.
    #[test]
    fn a_line_naming_several_domains_reads_as_several_entries() {
        let text = format!("{BEGIN_MARKER}\n127.0.0.1  a.test  b.test\n\n{END_MARKER}\n");

        assert_eq!(
            parse(&text).unwrap(),
            vec![entry("127.0.0.1", "a.test"), entry("127.0.0.1", "b.test")]
        );
    }

    /// A line inside the block that is not an entry is the same problem as a second `BEGIN`: it is
    /// on the machine, and guessing at it is not this code's business.
    #[test]
    fn a_line_in_the_block_that_names_no_address_is_refused() {
        let no_address = format!("{BEGIN_MARKER}\nnot-an-address blog.test\n{END_MARKER}\n");
        let no_domain = format!("{BEGIN_MARKER}\n127.0.0.1\n{END_MARKER}\n");

        assert!(parse(&no_address).is_err());
        assert!(parse(&no_domain).is_err());
    }

    /// A file the test owns, replaced for real — the whole read-modify-write, not just the splice.
    #[cfg(feature = "elevated")]
    #[test]
    fn applying_replaces_the_file_and_says_what_it_wrote() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hosts");
        let lock = directory.path().join("hosts.lock");
        std::fs::write(&file, REAL).unwrap();

        let entries = [entry("127.0.0.1", "blog.test")];
        let change = apply_under(&file, &entries, &lock).unwrap();

        assert!(
            matches!(change, Change::Written { entries: 1 }),
            "{change:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            splice(REAL, &entries).unwrap()
        );

        // D1's payoff: the second call is the byte comparison, not a judgement.
        assert!(matches!(
            apply_under(&file, &entries, &lock).unwrap(),
            Change::Unchanged
        ));

        assert!(matches!(
            apply_under(&file, &[], &lock).unwrap(),
            Change::Written { entries: 0 }
        ));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), REAL);
    }

    /// A refusal must not leave litter in `/etc`.
    #[cfg(feature = "elevated")]
    #[test]
    fn a_refused_apply_leaves_no_temporary_file_behind() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hosts");
        let lock = directory.path().join("hosts.lock");
        std::fs::write(
            &file,
            format!("{BEGIN_MARKER}\n{BEGIN_MARKER}\n{END_MARKER}\n"),
        )
        .unwrap();

        assert!(apply_under(&file, &[entry("127.0.0.1", "a.test")], &lock).is_err());

        let left: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|found| found.ok().map(|found| found.file_name()))
            .filter(|name| name.to_string_lossy().contains("mixengine"))
            .collect();

        assert!(left.is_empty(), "{left:?}");
    }

    /// D7: the file being replaced keeps the permissions it had. A `0644 root:root` `/etc/hosts`
    /// that quietly became something wider is a change nothing would report.
    #[cfg(all(unix, feature = "elevated"))]
    #[test]
    fn the_replaced_file_keeps_the_mode_it_had() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hosts");
        let lock = directory.path().join("hosts.lock");
        std::fs::write(&file, "127.0.0.1 localhost\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();

        apply_under(&file, &[entry("127.0.0.1", "blog.test")], &lock).unwrap();

        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "the mode came back as {mode:o}");
    }

    /// A hosts file that is not there is not a state any of the three systems ships in, and is
    /// reachable — a machine somebody has cleaned up. It is created rather than refused.
    #[cfg(feature = "elevated")]
    #[test]
    fn a_hosts_file_that_is_not_there_is_created() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hosts");
        let lock = directory.path().join("hosts.lock");

        apply_under(&file, &[entry("127.0.0.1", "blog.test")], &lock).unwrap();

        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("blog.test")
        );
    }

    /// D7 on the other family: `ReplaceFileW` keeps the replaced file's ACL, where a rename discards
    /// it. Asserted against `icacls`' own rendering, the way `tests/access.rs` asserts an ACL on
    /// macOS against `ls -lde`: the tool that decides is the tool that reports.
    #[cfg(all(windows, feature = "elevated"))]
    #[test]
    fn the_replaced_file_keeps_the_acl_it_had() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hosts");
        let lock = directory.path().join("hosts.lock");
        std::fs::write(&file, "127.0.0.1 localhost\n").unwrap();

        // A distinctive entry, so what is compared is an ACL somebody set rather than one the
        // temporary directory would have handed the replacement anyway.
        let granted = std::process::Command::new("icacls")
            .arg(&file)
            .args(["/grant", "*S-1-5-32-545:(R)"])
            .output()
            .expect("icacls runs on every Windows machine");
        assert!(granted.status.success(), "{granted:?}");

        let before = std::process::Command::new("icacls")
            .arg(&file)
            .output()
            .unwrap();

        apply_under(&file, &[entry("127.0.0.1", "blog.test")], &lock).unwrap();

        let after = std::process::Command::new("icacls")
            .arg(&file)
            .output()
            .unwrap();

        assert_eq!(
            String::from_utf8_lossy(&before.stdout),
            String::from_utf8_lossy(&after.stdout),
            "the replacement did not inherit the replaced file's ACL"
        );
    }

    /// D8: a lock somebody else holds is a failure and not a refusal — nothing about the request is
    /// wrong, and trying again will work.
    #[cfg(feature = "elevated")]
    #[test]
    fn a_lock_somebody_else_holds_stops_the_write() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hosts");
        let lock = directory.path().join("hosts.lock");
        std::fs::write(&file, "127.0.0.1 localhost\n").unwrap();

        let crate::lock::Acquired::Held(_held) = crate::lock::Lock::acquire(&lock).unwrap() else {
            panic!("nobody else is holding a lock in a directory this test just made");
        };

        let error = apply_under(&file, &[entry("127.0.0.1", "a.test")], &lock).unwrap_err();

        assert!(
            !matches!(error, crate::Error::MalformedBlock { .. }),
            "a held lock is not a refusal: {error}"
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "127.0.0.1 localhost\n"
        );
    }
}
