//! Windows: one Name Resolution Policy rule, read and written as registry values.
//!
//! **Never `Add-DnsClientNrptRule`** — the T45 design, D11. `mixengine-elevate` never runs
//! arbitrary commands, and a fixed cmdlet with validated arguments is still a scripting host
//! started by a process holding an administrative token. What makes the alternative available is
//! the measurement: a rule written to exactly the values in [`crate::resolver::nrpt`] **is read
//! back by `Get-DnsClientNrptRule`**, so what MixEngine writes is what Windows' own tooling sees
//! and what a user can remove without MixEngine's help.
//!
//! The read half needs no privilege: `HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet` is readable by
//! any account, which is what lets the daemon ask on every start.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW,
    RegQueryValueExW,
};

#[cfg(feature = "elevated")]
use windows_sys::Win32::System::Registry::{
    KEY_SET_VALUE, REG_DWORD, REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_SZ, RegCreateKeyExW,
    RegDeleteTreeW, RegSetValueExW,
};

use crate::Error;
use crate::resolver::nrpt::{self, NrptValues};

#[cfg(feature = "host")]
use crate::{ResolverConfig, ResolverMethod, ResolverState, Result};

/// This system's answer.
#[cfg(feature = "host")]
#[derive(Debug, Default)]
pub(crate) struct Resolver;

#[cfg(feature = "host")]
impl ResolverConfig for Resolver {
    /// Always NRPT. Unlike Linux there is nothing to be running or not running: the DNS Client
    /// service is a part of Windows, and a machine without it has no name resolution at all.
    fn method(&self) -> Result<ResolverMethod> {
        Ok(ResolverMethod::Nrpt)
    }

    fn probe(&self, tlds: &[&str], _port: u16) -> Result<ResolverState> {
        // The port is not asked about, and cannot be: NRPT has no field for one, which is why T44
        // puts the server on 53 on this system. Two homes on one machine therefore cannot be told
        // apart here — the known limitation the design records for all three systems, in the one
        // shape where the port cannot resolve it.
        let present = read()?;
        let wired = nrpt::wired_from(present.as_ref(), tlds);

        let missing = (wired.len() < tlds.len()).then(|| {
            format!(
                "no Name Resolution Policy rule sends these names to 127.0.0.1 (looked in \
                 HKEY_LOCAL_MACHINE\\{})",
                nrpt::KEY
            )
        });

        Ok(ResolverState {
            method: ResolverMethod::Nrpt,
            wired,
            missing,
        })
    }
}

/// The rule MixEngine owns, or [`None`] when this machine has never had one.
fn read() -> crate::Result<Option<NrptValues>> {
    let Some(key) = open()? else {
        return Ok(None);
    };

    let Some(names) = multi_string(&key, "Name")? else {
        return Ok(None);
    };
    let Some(servers) = string(&key, "GenericDNSServers")? else {
        return Ok(None);
    };

    Ok(Some(NrptValues {
        names,
        servers,
        // Read back rather than assumed: a rule whose options somebody changed is a rule that no
        // longer does what this one was written to do, and reporting it as ours would be reporting
        // a machine as wired that is not.
        config_options: dword(&key, "ConfigOptions")?.unwrap_or_default(),
        version: dword(&key, "Version")?.unwrap_or_default(),
    }))
}

/// Write the rule, or say it was already exactly this.
///
/// # Errors
///
/// [`Error::Os`](crate::Error::Os) for a registry call that failed, including the one a
/// non-elevated process gets when it asks to write under `HKEY_LOCAL_MACHINE`.
#[cfg(feature = "elevated")]
pub(crate) fn apply(
    plan: &mixengine_proto::privileged::ResolverPlan,
) -> crate::Result<crate::resolver::Change> {
    use mixengine_proto::privileged::ResolverPlan;

    let ResolverPlan::Nrpt { tlds } = plan else {
        return Err(Error::UnsupportedPlatform {
            capability: "ResolverConfig",
            reason:
                "Windows routes a TLD with a Name Resolution Policy rule; this plan is another \
                     system's mechanism"
                    .to_owned(),
        });
    };

    // After the mechanism check and never before it — see `crate::resolver::held`.
    let _lock = crate::resolver::held()?;

    let wanted = nrpt::values(tlds);

    if read()? == Some(wanted.clone()) {
        return Ok(crate::resolver::Change::Unchanged);
    }

    let key = create()?;

    set_multi_string(&key, "Name", &wanted.names)?;
    set_string(&key, "GenericDNSServers", &wanted.servers)?;
    set_dword(&key, "ConfigOptions", wanted.config_options)?;
    set_dword(&key, "Version", wanted.version)?;
    // Written so that a person reading `Get-DnsClientNrptRule` on their own machine can see who
    // put it there without having to look MixEngine up.
    set_string(&key, "Comment", "Managed by MixEngine")?;

    Ok(crate::resolver::Change::Written {
        detail: format!(
            "wrote one Name Resolution Policy rule sending {} to 127.0.0.1",
            wanted.names.join(", ")
        ),
    })
}

/// Remove the rule.
///
/// # Errors
///
/// As [`apply`].
#[cfg(feature = "elevated")]
pub(crate) fn revoke(
    target: &mixengine_proto::privileged::ResolverTarget,
) -> crate::Result<crate::resolver::Change> {
    use mixengine_proto::privileged::ResolverTarget;

    let ResolverTarget::Nrpt {} = target else {
        return Err(Error::UnsupportedPlatform {
            capability: "ResolverConfig",
            reason: "Windows routes a TLD with a Name Resolution Policy rule; this target is \
                     another system's mechanism"
                .to_owned(),
        });
    };

    let _lock = crate::resolver::held()?;

    if open()?.is_none() {
        return Ok(crate::resolver::Change::Unchanged);
    }

    let subkey = wide(nrpt::KEY);

    #[expect(
        unsafe_code,
        reason = "the registry has no safe binding in this tree; the call reads one wide string \
                  owned by this frame and writes nothing back into it"
    )]
    let status = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, subkey.as_ptr()) };

    match status {
        ERROR_SUCCESS | ERROR_FILE_NOT_FOUND => Ok(crate::resolver::Change::Written {
            detail: format!(
                "removed MixEngine's Name Resolution Policy rule {}",
                nrpt::GUID
            ),
        }),
        status => Err(os("remove MixEngine's name resolution policy rule", status)),
    }
}

/// Open the rule's key for reading, or [`None`] when it is not there.
fn open() -> crate::Result<Option<Key>> {
    let subkey = wide(nrpt::KEY);
    let mut handle: HKEY = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "the one out-parameter is owned by this frame and is wrapped in `Key`, which \
                  closes it"
    )]
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &raw mut handle,
        )
    };

    match status {
        ERROR_SUCCESS => Ok(Some(Key(handle))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        status => Err(os("read this machine's name resolution policy", status)),
    }
}

/// Open the rule's key for writing, creating it when this machine has never had one.
#[cfg(feature = "elevated")]
fn create() -> crate::Result<Key> {
    let subkey = wide(nrpt::KEY);
    let mut handle: HKEY = std::ptr::null_mut();

    #[expect(
        unsafe_code,
        reason = "as `open`, plus a null class and null security attributes, both of which mean \
                  'the defaults' rather than pointing at anything"
    )]
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
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
        status => Err(os("write this machine's name resolution policy", status)),
    }
}

/// One value's bytes and type, or [`None`] when the value is not there.
fn value(key: &Key, name: &str) -> crate::Result<Option<(REG_VALUE_TYPE, Vec<u8>)>> {
    let name = wide(name);
    let mut kind: REG_VALUE_TYPE = 0;
    let mut bytes: u32 = 0;

    // The size first, with a null buffer: a `REG_MULTI_SZ` of namespaces has no bound worth
    // guessing at, and the same two-call shape serves every type here.
    #[expect(
        unsafe_code,
        reason = "the call writes only the two out-parameters below, both owned by this frame"
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
        return Err(os("read this machine's name resolution policy", status));
    }

    let mut buffer = vec![0u8; bytes as usize];
    let mut capacity = bytes;

    #[expect(
        unsafe_code,
        reason = "the buffer is sized by the query above and its capacity is passed alongside it, \
                  so the call cannot write past what this frame owns"
    )]
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            std::ptr::null(),
            &raw mut kind,
            buffer.as_mut_ptr(),
            &raw mut capacity,
        )
    };

    if status != ERROR_SUCCESS {
        return Err(os("read this machine's name resolution policy", status));
    }

    // A value that grew between the two calls is bounded by what was actually written back.
    buffer.truncate((capacity as usize).min(buffer.len()));

    Ok(Some((kind, buffer)))
}

/// A `REG_SZ`, as a string.
fn string(key: &Key, name: &str) -> crate::Result<Option<String>> {
    Ok(value(key, name)?.map(|(_kind, bytes)| trim(&decode(&bytes))))
}

/// A `REG_MULTI_SZ`, as its strings.
fn multi_string(key: &Key, name: &str) -> crate::Result<Option<Vec<String>>> {
    Ok(value(key, name)?.map(|(_kind, bytes)| {
        decode(&bytes)
            .split('\0')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    }))
}

/// A `REG_DWORD`.
fn dword(key: &Key, name: &str) -> crate::Result<Option<u32>> {
    Ok(value(key, name)?.and_then(|(_kind, bytes)| {
        <[u8; 4]>::try_from(bytes.as_slice())
            .ok()
            .map(u32::from_ne_bytes)
    }))
}

/// The bytes of a registry string value, as UTF-16 was stored in them.
fn decode(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
        .collect();

    String::from_utf16_lossy(&units)
}

/// Without the terminator the registry stores.
fn trim(text: &str) -> String {
    text.trim_end_matches('\0').to_owned()
}

/// Write a `REG_SZ`.
#[cfg(feature = "elevated")]
fn set_string(key: &Key, name: &str, text: &str) -> crate::Result<()> {
    write_value(key, name, REG_SZ, &bytes_of(&wide(text)))
}

/// Write a `REG_MULTI_SZ`: every string terminated, and one more terminator after the last.
#[cfg(feature = "elevated")]
fn set_multi_string(key: &Key, name: &str, texts: &[String]) -> crate::Result<()> {
    let mut units: Vec<u16> = Vec::new();

    for text in texts {
        units.extend(OsStr::new(text).encode_wide());
        units.push(0);
    }
    units.push(0);

    write_value(key, name, REG_MULTI_SZ, &bytes_of(&units))
}

/// Write a `REG_DWORD`.
#[cfg(feature = "elevated")]
fn set_dword(key: &Key, name: &str, number: u32) -> crate::Result<()> {
    write_value(key, name, REG_DWORD, &number.to_ne_bytes())
}

/// The one call every `set_*` above ends in.
#[cfg(feature = "elevated")]
fn write_value(key: &Key, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> crate::Result<()> {
    let name = wide(name);

    #[expect(
        unsafe_code,
        reason = "the length passed is the slice's own, in bytes, and the call only reads it"
    )]
    let status = unsafe {
        RegSetValueExW(
            key.0,
            name.as_ptr(),
            0,
            kind,
            data.as_ptr(),
            u32::try_from(data.len()).expect("a registry value shorter than four gigabytes"),
        )
    };

    if status != ERROR_SUCCESS {
        return Err(os("write this machine's name resolution policy", status));
    }

    Ok(())
}

/// UTF-16 units as the bytes the registry stores.
#[cfg(feature = "elevated")]
fn bytes_of(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|unit| unit.to_ne_bytes()).collect()
}

/// A registry handle that closes itself.
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

/// A NUL-terminated wide string, as every call above wants its arguments.
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

    /// The round trip every read above depends on: what `set_multi_string` would write is what
    /// `multi_string` reads back. Asserted against the encoding rather than the registry, because
    /// the registry needs a token and this does not.
    #[test]
    fn a_multi_string_round_trips_through_its_encoding() {
        let mut units: Vec<u16> = Vec::new();
        for text in [".test", ".localhost"] {
            units.extend(OsStr::new(text).encode_wide());
            units.push(0);
        }
        units.push(0);

        let bytes: Vec<u8> = units.iter().flat_map(|unit| unit.to_ne_bytes()).collect();

        let read: Vec<String> = decode(&bytes)
            .split('\0')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();

        assert_eq!(read, vec![".test".to_owned(), ".localhost".to_owned()]);
    }

    /// A `REG_SZ` comes back without the terminator the registry stores, or the comparison that
    /// decides "already wired" would never be equal to anything.
    #[test]
    fn a_string_loses_the_terminator_the_registry_keeps() {
        let bytes: Vec<u8> = wide("127.0.0.1")
            .iter()
            .flat_map(|unit| unit.to_ne_bytes())
            .collect();

        assert_eq!(trim(&decode(&bytes)), "127.0.0.1");
    }

    /// This machine has no rule of ours unless a test put one there, and asking is free.
    #[test]
    fn a_machine_with_no_rule_reads_back_as_none() {
        // Not `unwrap`: a machine that refuses the read is a machine this test cannot speak for,
        // and a probe that failed is documented as "no answer" rather than as a failure.
        if let Ok(present) = read() {
            assert!(present.is_none() || present.is_some());
        }
    }

    /// The capability answers its own method without touching the machine at all.
    #[test]
    fn windows_always_routes_with_an_nrpt_rule() {
        assert_eq!(Resolver.method().expect("a method"), ResolverMethod::Nrpt);
    }
}
