//! `security.capability`, the extended attribute a Linux file carries its capabilities in.
//!
//! **No `libcap`.** `getcap` and `setcap` come from a package that is not guaranteed present, and
//! the read side runs on every daemon start on every Linux machine, so it may not depend on one
//! being installed — the T42 design, D8. Once `getxattr` is being called, `setxattr` is the same
//! quantity of `unsafe` for the write side, so both directions are syscalls and the format is
//! decoded here.
//!
//! **Compiled on all three systems** so the codec is tested on all three, exactly as the elevation
//! launchers' tables are: this is a byte layout with nothing OS-specific in it beyond the kernel
//! that reads it. The two syscalls are in `linux/port_access.rs`.
//!
//! The layout is `struct vfs_cap_data` from `linux/capability.h`: a little-endian `magic_etc`, then
//! a `permitted` and an `inheritable` mask for each of the two 32-bit halves of the capability set.
//! Revision 1 stops after the first half (12 bytes), revision 2 carries both (20), and revision 3
//! appends a `rootid` (24) without moving anything before it.

#![allow(
    dead_code,
    reason = "the codec is compiled on all three systems and called on one, which is the module's               whole purpose: on Windows and macOS it is read by the tests below and by nothing else"
)]

/// The attribute's name.
pub(crate) const XATTR: &str = "security.capability";

/// `CAP_NET_BIND_SERVICE` is capability 10 — `linux/capability.h`.
const NET_BIND_SERVICE: u32 = 1 << 10;

/// `VFS_CAP_REVISION_1`, in the top byte of `magic_etc`.
const REVISION_1: u32 = 0x0100_0000;

/// `VFS_CAP_REVISION_2`, in the top byte of `magic_etc`.
const REVISION_2: u32 = 0x0200_0000;

/// `VFS_CAP_REVISION_3`, which appends a `rootid` and moves nothing before it.
const REVISION_3: u32 = 0x0300_0000;

/// Where the revision lives, so 1 and 3 are recognised rather than refused.
const REVISION: u32 = 0xFF00_0000;

/// `VFS_CAP_FLAGS_EFFECTIVE`. Without it a capability is permitted but not raised, and a program
/// that does not call `capset` itself — which is every front end there is — gets nothing from it.
const EFFECTIVE: u32 = 0x0000_0001;

/// A revision-2 `vfs_cap_data` granting `cap_net_bind_service`, effective: `cap_net_bind_service=+ep`.
pub(crate) const ENCODED: [u8; 20] = {
    let mut raw = [0u8; 20];

    let magic = (REVISION_2 | EFFECTIVE).to_le_bytes();
    let permitted = NET_BIND_SERVICE.to_le_bytes();

    raw[0] = magic[0];
    raw[1] = magic[1];
    raw[2] = magic[2];
    raw[3] = magic[3];
    raw[4] = permitted[0];
    raw[5] = permitted[1];
    raw[6] = permitted[2];
    raw[7] = permitted[3];

    raw
};

/// Does `raw` grant `cap_net_bind_service`, raised?
///
/// Three questions rather than a byte comparison against [`ENCODED`]: a file may carry a revision
/// this build did not write, more capabilities than this one, or the same one without the effective
/// flag — and only the last of those is a no.
pub(crate) fn grants_bind(raw: &[u8]) -> bool {
    let (Some(magic), Some(permitted)) = (word(raw, 0), word(raw, 4)) else {
        return false;
    };

    let revision = magic & REVISION;
    let known = revision == REVISION_1 || revision == REVISION_2 || revision == REVISION_3;

    known && magic & EFFECTIVE != 0 && permitted & NET_BIND_SERVICE != 0
}

/// The little-endian `u32` at `at`, or [`None`] when `raw` is shorter than that.
fn word(raw: &[u8], at: usize) -> Option<u32> {
    raw.get(at..at + 4)?.try_into().ok().map(u32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact twenty bytes, asserted as a literal. A change to any of the constants above is a
    /// change to what the kernel is handed, and it should be visible here rather than inferred.
    #[test]
    fn the_encoding_is_what_setcap_writes() {
        assert_eq!(
            ENCODED,
            [
                // magic_etc: VFS_CAP_REVISION_2 | VFS_CAP_FLAGS_EFFECTIVE, little-endian.
                0x01, 0x00, 0x00, 0x02, //
                // data[0].permitted: 1 << CAP_NET_BIND_SERVICE.
                0x00, 0x04, 0x00, 0x00, //
                // data[0].inheritable, then the whole of data[1].
                0x00, 0x00, 0x00, 0x00, //
                0x00, 0x00, 0x00, 0x00, //
                0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    /// The round trip the probe depends on: what this writes, it reads back as a grant.
    #[test]
    fn what_this_writes_reads_back_as_a_grant() {
        assert!(grants_bind(&ENCODED));
    }

    /// A file may hold a capability that is not the one being asked about. `getcap` printing
    /// *something* is not the question — the question is whether port 80 can be bound.
    #[test]
    fn a_capability_that_is_not_the_one_wanted_is_not_a_grant() {
        let mut other = ENCODED;
        // CAP_NET_RAW is 13, and it is not what a front end needs.
        other[4..8].copy_from_slice(&(1u32 << 13).to_le_bytes());

        assert!(!grants_bind(&other));
    }

    /// Permitted but not effective is a capability a program has to raise with `capset` itself.
    /// Caddy and nginx do not, so it is worth nothing and must not read as a grant.
    #[test]
    fn a_permitted_capability_that_is_not_effective_is_not_a_grant() {
        let mut inert = ENCODED;
        inert[0] = 0x00;

        assert!(!grants_bind(&inert));
    }

    /// A revision-1 attribute is twelve bytes and is still a grant. A kernel or a `setcap` older
    /// than the machine is not a reason to ask for a prompt that would change nothing.
    #[test]
    fn a_revision_one_attribute_is_read() {
        let mut old = [0u8; 12];
        old[0..4].copy_from_slice(&0x0100_0001u32.to_le_bytes());
        old[4..8].copy_from_slice(&(1u32 << 10).to_le_bytes());

        assert!(grants_bind(&old));
    }

    /// No attribute, a truncated one, and a filesystem that does not carry them: all the same
    /// answer, which is that nothing is granted.
    #[test]
    fn nothing_and_rubbish_are_both_not_a_grant() {
        assert!(!grants_bind(&[]));
        assert!(!grants_bind(&ENCODED[..7]));
        assert!(!grants_bind(&[0xff; 20]));
    }
}
