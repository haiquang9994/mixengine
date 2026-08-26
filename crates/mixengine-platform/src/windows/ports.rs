//! Who is listening on a local TCP port, on Windows.
//!
//! `GetExtendedTcpTable` with `TCP_TABLE_OWNER_PID_LISTENER`, which is the listening half of the
//! table `netstat -ano` prints and comes with the owning pid already attached. Shelling out to
//! `netstat` would mean parsing a localised, column-aligned page of text for a number the API hands
//! over as a `DWORD`.
//!
//! **Both address families are asked, in that order.** A server bound to `0.0.0.0` appears only in
//! the IPv4 table and one bound to `[::]` only in the IPv6 one — and on Windows a dual-stack socket
//! is the second of those, so a lookup that asked about IPv4 alone would report nothing at all for
//! a modern server holding the port.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt as _;
use std::path::PathBuf;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP_STATE_ESTAB, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID,
    MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_CLASS, TCP_TABLE_OWNER_PID_ALL,
    TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

use crate::{Error, PortHolder, PortOwner, Result};

/// How many times a table that keeps growing is re-read before the answer is given up on.
///
/// The size is asked for and the table is fetched in two separate calls, so a machine that opens a
/// socket in between makes the second one fail with the same "too small" the first did. Three
/// attempts rather than a loop: a table that has grown twice while being read belongs to a machine
/// whose listening set is churning, and no diagnosis of one port is worth spinning for.
const ATTEMPTS: usize = 3;

/// What was being attempted, for the one error this module raises.
const ACTION: &str = "ask which process is listening on a port";

/// `MIB_TCP_STATE_ESTAB`, widened once here rather than at each of the two rows that compare it.
///
/// The header declares the states as a signed enum and declares `dwState` as a `DWORD`, so the
/// comparison needs one of them converted whatever happens; doing it once, in a `const`, keeps the
/// conversion out of a hot filter and out of two `#[expect]`s.
const ESTABLISHED: u32 = MIB_TCP_STATE_ESTAB.cast_unsigned();

#[derive(Debug)]
pub(crate) struct Ports;

impl PortOwner for Ports {
    fn listening_on(&self, port: u16) -> Result<Option<PortHolder>> {
        let pid = match owner(Family::V4, port)? {
            Some(pid) => Some(pid),
            None => owner(Family::V6, port)?,
        };

        Ok(pid.map(|pid| PortHolder {
            pid: Some(pid),
            name: name_of(pid),
        }))
    }
}

impl crate::ConnectionCount for Ports {
    fn established_on(&self, port: u16) -> Result<usize> {
        // Summed rather than stopped at the first answer, which is where this differs from
        // `listening_on` above: a port has one holder and any number of clients, and a dual-stack
        // server's are spread across both tables.
        Ok(established(Family::V4, port)? + established(Family::V6, port)?)
    }
}

/// How many connections are established to `port` in this family's table.
///
/// `TCP_TABLE_OWNER_PID_ALL` rather than `_LISTENER`, and with it a filter the listener path needs
/// no equivalent of: every row of the listening table is a listener, while this one holds every
/// socket in every state — `SYN_SENT`, `TIME_WAIT`, a connection being torn down. Only
/// `MIB_TCP_STATE_ESTAB` means somebody is on the other end right now.
fn established(family: Family, port: u16) -> Result<usize> {
    let buffer = table(family, TCP_TABLE_OWNER_PID_ALL)?;

    let count = match family {
        #[expect(
            unsafe_code,
            reason = "the buffer was filled by GetExtendedTcpTable for this family, so it holds \
                      one MIB_TCPTABLE_OWNER_PID followed by dwNumEntries MIB_TCPROW_OWNER_PID"
        )]
        Family::V4 => unsafe {
            let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let rows = std::slice::from_raw_parts(
                (&raw const (*table).table).cast::<MIB_TCPROW_OWNER_PID>(),
                (*table).dwNumEntries as usize,
            );

            rows.iter()
                .filter(|row| row.dwState == ESTABLISHED && local_port(row.dwLocalPort) == port)
                .count()
        },

        #[expect(
            unsafe_code,
            reason = "as the arm above, for the family whose rows are MIB_TCP6ROW_OWNER_PID"
        )]
        Family::V6 => unsafe {
            let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let rows = std::slice::from_raw_parts(
                (&raw const (*table).table).cast::<MIB_TCP6ROW_OWNER_PID>(),
                (*table).dwNumEntries as usize,
            );

            rows.iter()
                .filter(|row| row.dwState == ESTABLISHED && local_port(row.dwLocalPort) == port)
                .count()
        },
    };

    Ok(count)
}

/// Which of the two listening tables to read.
///
/// The rows have different shapes, so the two are read by different code; this exists so that only
/// the reading differs and the retry, the sizing and the error do not.
#[derive(Clone, Copy)]
enum Family {
    V4,
    V6,
}

impl Family {
    /// What `GetExtendedTcpTable` calls it.
    fn af(self) -> u32 {
        match self {
            Self::V4 => u32::from(AF_INET),
            Self::V6 => u32::from(AF_INET6),
        }
    }
}

/// The pid listening on `port` in this family's table, if one is.
fn owner(family: Family, port: u16) -> Result<Option<u32>> {
    let buffer = table(family, TCP_TABLE_OWNER_PID_LISTENER)?;

    let owner = match family {
        #[expect(
            unsafe_code,
            reason = "the buffer was filled by GetExtendedTcpTable for this family, so it holds \
                      one MIB_TCPTABLE_OWNER_PID followed by dwNumEntries MIB_TCPROW_OWNER_PID"
        )]
        Family::V4 => unsafe {
            let table = buffer.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>();
            let rows = std::slice::from_raw_parts(
                (&raw const (*table).table).cast::<MIB_TCPROW_OWNER_PID>(),
                (*table).dwNumEntries as usize,
            );

            rows.iter()
                .find(|row| local_port(row.dwLocalPort) == port)
                .map(|row| row.dwOwningPid)
        },

        #[expect(
            unsafe_code,
            reason = "as the arm above, for the family whose rows are MIB_TCP6ROW_OWNER_PID"
        )]
        Family::V6 => unsafe {
            let table = buffer.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>();
            let rows = std::slice::from_raw_parts(
                (&raw const (*table).table).cast::<MIB_TCP6ROW_OWNER_PID>(),
                (*table).dwNumEntries as usize,
            );

            rows.iter()
                .find(|row| local_port(row.dwLocalPort) == port)
                .map(|row| row.dwOwningPid)
        },
    };

    Ok(owner)
}

/// One of a family's tables, in a buffer aligned for the rows it holds.
///
/// `Vec<u32>` rather than `Vec<u8>` for that alignment: every `MIB_*` type here is `DWORD`s and one
/// `[u8; 16]`, so a buffer aligned for `u32` is aligned for all of them, and a byte vector would be
/// aligned for none.
///
/// `class` selects which table — `TCP_TABLE_OWNER_PID_LISTENER` for who holds a port,
/// `TCP_TABLE_OWNER_PID_ALL` for how busy one is. The two rows have the same shape, so only the
/// class differs and the retry, the sizing and the error are shared.
fn table(family: Family, class: TCP_TABLE_CLASS) -> Result<Vec<u32>> {
    let mut size: u32 = 0;
    let mut buffer: Vec<u32> = Vec::new();

    for _ in 0..ATTEMPTS {
        #[expect(
            unsafe_code,
            reason = "the buffer and the size are locals of this function, and the call writes at \
                      most `size` bytes into a buffer allocated for exactly that many"
        )]
        let status = unsafe {
            GetExtendedTcpTable(
                if buffer.is_empty() {
                    std::ptr::null_mut()
                } else {
                    buffer.as_mut_ptr().cast()
                },
                &raw mut size,
                0,
                family.af(),
                class,
                0,
            )
        };

        if status == NO_ERROR && !buffer.is_empty() {
            return Ok(buffer);
        }

        if status != NO_ERROR && status != ERROR_INSUFFICIENT_BUFFER {
            return Err(failed(status));
        }

        // Rounded up, because the API counts bytes and this counts `u32`s. At least one, so that a
        // machine with nothing listening at all still gets a buffer and the second call reports the
        // empty table rather than being asked to size it again.
        buffer = vec![0; (size as usize).div_ceil(size_of::<u32>()).max(1)];
    }

    Err(failed(ERROR_INSUFFICIENT_BUFFER))
}

/// The one error this module raises, from a Win32 status.
fn failed(status: u32) -> Error {
    Error::Os {
        action: ACTION,
        #[expect(
            clippy::cast_possible_wrap,
            reason = "a Win32 error code is what from_raw_os_error takes, and it takes it signed"
        )]
        source: io::Error::from_raw_os_error(status as i32),
    }
}

/// The port out of a `dwLocalPort`, which carries it in network order in its low word.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the high word of dwLocalPort is documented as zero; the port is the low one"
)]
fn local_port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

/// The file name of the program `pid` is running, where this account may ask.
///
/// `None` rather than an error on every refusal, and there are several: pid 0 and `System` cannot
/// be opened at all, and a process belonging to another account is opened only by somebody holding
/// `SeDebugPrivilege`. That is [`PortHolder::name`]'s documented case rather than a fault — and it
/// is exactly what a Windows service holding 3306 looks like from an ordinary account, which is the
/// most likely thing this whole module will ever be pointed at.
fn name_of(pid: u32) -> Option<String> {
    #[expect(
        unsafe_code,
        reason = "OpenProcess takes no pointer, and the handle it returns is closed below on every \
                  path out of this function"
    )]
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };

    if process.is_null() {
        return None;
    }

    // Wide enough for a long path, which is what the full image name may be; the call writes back
    // how much of it was used.
    let mut buffer = vec![0u16; 4096];
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the buffer is 4096 wide characters, which is far inside a u32"
    )]
    let mut written = buffer.len() as u32;

    #[expect(
        unsafe_code,
        reason = "the handle is the one opened above, and both pointers are locals sized by the \
                  length passed with them"
    )]
    let named = unsafe {
        let named = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &raw mut written);
        CloseHandle(process);
        named
    };

    if named == 0 {
        return None;
    }

    let path = PathBuf::from(OsString::from_wide(&buffer[..written as usize]));

    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}
