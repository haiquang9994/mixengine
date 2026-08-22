//! Raising UAC on the one-shot helper.
//!
//! `ShellExecuteExW` with the `runas` verb, which is the only documented way to start a process under
//! a token this one does not hold: `CreateProcessAsUser` needs a privilege the daemon does not have,
//! and marking the *helper* `requireAdministrator` in a manifest would make it un-runnable
//! unelevated, which T40's own protocol tests depend on being able to do.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use mixengine_proto::privileged::ElevationOutcome;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use crate::prompt::{self, windows as decide};
use crate::{Elevation, ElevationSupport, Error, Result};

/// The verb that asks for a token this process does not have.
const RUNAS: &str = "runas";

#[derive(Debug)]
pub(crate) struct Prompt;

impl Elevation for Prompt {
    fn probe(&self) -> ElevationSupport {
        // Unconditionally, and deliberately. UAC is part of the OS, and on an account with no
        // administrative rights the prompt asks for somebody else's credentials rather than being
        // absent — so there is nothing cheap to look at here that would be true. See T40a, D6.
        ElevationSupport::Available
    }

    fn run(&self, helper: &Path, request: &Path) -> Result<ElevationOutcome> {
        prompt::usable("run as the elevation helper", helper)?;

        // Before the request is looked for, not after: a Windows path cannot contain a quotation
        // mark, so "there is no file there" would name the wrong problem for one that does.
        let parameters = decide::parameters(request)?;
        prompt::usable("hand to the elevation helper", request)?;

        // The helper is an existing file, so it has a parent, and that parent is the root-owned
        // directory the installer chose. **A null `lpDirectory` would give the elevated child the
        // daemon's own working directory**, which the user controls and which Windows searches when
        // it resolves a DLL — an elevated process with its working directory in a user-writable
        // place is a DLL-planting target, and the fix costs one field. See T40a, D4.
        let directory = helper.parent().unwrap_or_else(|| Path::new("\\"));

        let verb = wide(OsStr::new(RUNAS));
        let file = wide(helper.as_os_str());
        let parameters = wide(&parameters);
        let directory = wide(directory.as_os_str());

        // `ShellExecuteExW` documents an initialised apartment on the calling thread.
        let _apartment = Apartment::entered();

        #[expect(
            unsafe_code,
            reason = "SHELLEXECUTEINFOW is a plain C struct of integers and pointers, and the API \
                      requires every field it is not given to be zero"
        )]
        let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };

        info.cbSize = u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>())
            .expect("a struct of a few pointers fits in a u32");
        // `NOCLOSEPROCESS` for a handle to wait on; `NOASYNC` because the calling thread has no
        // message loop and may exit before the shell is finished with it; `FLAG_NO_UI` so a failure
        // comes back as an error code rather than as a dialog on a machine nobody is looking at.
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = parameters.as_ptr();
        info.lpDirectory = directory.as_ptr();
        // The helper is a console program with nothing to show; UAC's own dialog is not this window.
        info.nShow = SW_HIDE;

        #[expect(
            unsafe_code,
            reason = "`info` is a local, every field is set above, and each string it points at is a \
                      live local for the length of the call"
        )]
        let raised = unsafe { ShellExecuteExW(&raw mut info) };

        if raised == 0 {
            let refusal = std::io::Error::last_os_error();

            // The person dismissed the prompt, and the helper never started — which is exactly why
            // T40/D11 said a declined prompt could not be an exit code of the helper's.
            if refusal.raw_os_error()
                == Some(i32::try_from(decide::ERROR_CANCELLED).expect("1223 fits in an i32"))
            {
                return Ok(ElevationOutcome::Declined);
            }

            return Err(Error::Os {
                action: "raise the elevation prompt",
                source: refusal,
            });
        }

        Ok(finished(info.hProcess))
    }
}

/// Wait for the elevated child, and report that it ran.
///
/// The exit code is logged and goes no further. Two of the three systems hand one back cheaply and
/// the third does not, so a caller that branched on it would be branching on something one of them
/// cannot supply — T40a, D7. The report is beside the request.
fn finished(process: HANDLE) -> ElevationOutcome {
    if process.is_null() {
        // `SEE_MASK_NOCLOSEPROCESS` asked for a handle; a shell that satisfied the request with a
        // process that already existed hands back none, and there is then nothing to wait on.
        tracing::debug!("the elevation prompt returned no process handle to wait on");
        return ElevationOutcome::Completed;
    }

    #[expect(
        unsafe_code,
        reason = "`process` came from ShellExecuteExW above and is closed below"
    )]
    unsafe {
        WaitForSingleObject(process, INFINITE);
    }

    let mut code: u32 = 0;

    #[expect(unsafe_code, reason = "as above; `code` is a local")]
    let read = unsafe { GetExitCodeProcess(process, &raw mut code) };

    #[expect(unsafe_code, reason = "the handle is ours and is not used again")]
    unsafe {
        CloseHandle(process);
    }

    if read == 0 {
        tracing::debug!("the elevated helper ended and Windows would not say with what code");
    } else {
        tracing::debug!(exit_code = code, "the elevated helper ended");
    }

    ElevationOutcome::Completed
}

/// A COM apartment for the length of one call.
///
/// A thread already in an apartment of another kind answers `RPC_E_CHANGED_MODE`, which is not a
/// failure — but it must not be paired with a `CoUninitialize` that would tear down somebody else's.
/// That is the whole of what the flag inside records.
struct Apartment(bool);

impl Apartment {
    fn entered() -> Self {
        #[expect(
            unsafe_code,
            reason = "no reserved pointer is passed, and the matching CoUninitialize is this type's \
                      Drop"
        )]
        let entered = unsafe {
            CoInitializeEx(
                std::ptr::null(),
                u32::try_from(COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
                    .expect("two small flags"),
            )
        };

        // Every non-negative HRESULT is a success this thread now owns; a negative one is either
        // RPC_E_CHANGED_MODE or a failure, and neither is ours to undo.
        Self(entered >= 0)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            #[expect(
                unsafe_code,
                reason = "paired with the CoInitializeEx that succeeded in `entered`"
            )]
            unsafe {
                CoUninitialize();
            }
        }
    }
}

/// A null-terminated UTF-16 string, which is what every `PCWSTR` above wants.
fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    /// `crate::prompt::windows` spells 1223 out so the table can be read on a machine that is not
    /// Windows. This is what stops the two from drifting.
    #[test]
    fn the_cancelled_error_is_the_one_the_sdk_names() {
        assert_eq!(
            super::decide::ERROR_CANCELLED,
            windows_sys::Win32::Foundation::ERROR_CANCELLED
        );
    }
}
