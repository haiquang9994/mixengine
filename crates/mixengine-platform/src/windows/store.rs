//! The certificate store itself: opening it, walking it, and — under an administrative token —
//! changing it.
//!
//! **Every unsafe call in T49a is in this file**, which is the arrangement the rest of `windows/`
//! already uses: the reason to keep them together is that a person auditing what this project does
//! with raw pointers can read one file rather than three.
//!
//! **The handle owns its close.** `CertCloseStore` and `CertFreeCertificateContext` are easy to
//! miss on an early return, so the store is a type with a `Drop` and the enumeration never hands a
//! context out — a caller gets the bytes and the context is freed before the next one is fetched.

use std::ffi::c_void;

use windows_sys::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_STORE_PROV_SYSTEM_W, CERT_STORE_READONLY_FLAG,
    CERT_SYSTEM_STORE_LOCAL_MACHINE, CertCloseStore, CertEnumCertificatesInStore, CertOpenStore,
    HCERTSTORE, X509_ASN_ENCODING,
};

/// `Root`, as a null-terminated wide string, which is what `CertOpenStore` reads `pvPara` as.
///
/// A constant and never a value from a request: a store name that travelled would be a request that
/// could choose which of this machine's stores to write.
const ROOT: &[u16] = &[b'R' as u16, b'O' as u16, b'O' as u16, b'T' as u16, 0];

/// An open store, closed when it goes out of scope.
struct Store(HCERTSTORE);

impl Drop for Store {
    fn drop(&mut self) {
        // # Safety
        //
        // `self.0` came from a `CertOpenStore` that returned non-null and has not been closed —
        // this type is the only thing that closes one, and it is not `Clone`.
        #[expect(
            unsafe_code,
            reason = "closing a store handle has no safe equivalent, and leaking one from a daemon \
                      that probes on every start is a handle leak per start"
        )]
        unsafe {
            CertCloseStore(self.0, 0);
        }
    }
}

/// Open `LocalMachine\Root`.
///
/// `read_only` is what the daemon asks for and what `tests/trust.rs` proves an ordinary account may
/// do; the helper asks for a writable one and holds a token that permits it.
fn open(read_only: bool) -> crate::Result<Store> {
    let flags = CERT_SYSTEM_STORE_LOCAL_MACHINE
        | if read_only {
            CERT_STORE_READONLY_FLAG
        } else {
            0
        };

    // # Safety
    //
    // `CERT_STORE_PROV_SYSTEM_W` is the provider that reads `pvPara` as a wide string, and `ROOT`
    // is a `&'static [u16]` with its own terminator. Nothing here outlives the call.
    #[expect(
        unsafe_code,
        reason = "opening a certificate store has no safe equivalent in this crate's dependency \
                  budget, and `certutil.exe` was refused in the T49a design's D6"
    )]
    let handle = unsafe {
        CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            X509_ASN_ENCODING,
            0,
            flags,
            ROOT.as_ptr().cast::<c_void>(),
        )
    };

    if handle.is_null() {
        return Err(crate::Error::Os {
            action: "open this machine's Trusted Root Certification Authorities",
            source: std::io::Error::last_os_error(),
        });
    }

    Ok(Store(handle))
}

/// Is there a certificate in the store whose bytes `matches` accepts?
///
/// **The context never leaves this function.** A caller gets a `&[u8]` valid for the length of its
/// own call and the context is freed before the next one is fetched, so there is no way to hold a
/// pointer into a store that has since been closed.
///
/// Reading is the producer's half of T49a and the helper never does it, so this is `host`'s: the
/// elevated build reaches the store through [`add`] and [`remove`] instead.
#[cfg(feature = "host")]
pub(crate) fn each_certificate(mut matches: impl FnMut(&[u8]) -> bool) -> crate::Result<bool> {
    use windows_sys::Win32::Security::Cryptography::CertFreeCertificateContext;

    let store = open(true)?;
    let mut previous: *mut CERT_CONTEXT = std::ptr::null_mut();

    loop {
        // # Safety
        //
        // `store.0` is open for the whole loop. `CertEnumCertificatesInStore` takes the previous
        // context and **frees it itself**, which is why `previous` is not freed here and why it is
        // replaced rather than reused. A null `previous` starts the enumeration.
        #[expect(
            unsafe_code,
            reason = "walking a certificate store has no safe equivalent, and the alternative was \
                      spawning `certutil.exe` from a process holding an administrative token"
        )]
        let context = unsafe { CertEnumCertificatesInStore(store.0, previous) };

        if context.is_null() {
            // The end of the store, which is also what an empty store answers immediately.
            return Ok(false);
        }

        // # Safety
        //
        // `context` is non-null and was just returned by the enumeration, so its two fields describe
        // a buffer the store owns and keeps alive until the context is freed. The slice does not
        // outlive this iteration.
        #[expect(
            unsafe_code,
            reason = "the encoded certificate is a pointer and a length, and reading it is the \
                      whole point of the enumeration"
        )]
        let found = unsafe {
            std::slice::from_raw_parts((*context).pbCertEncoded, (*context).cbCertEncoded as usize)
        };

        if matches(found) {
            // # Safety
            //
            // `context` is the live context from this iteration and is not used again: the
            // enumeration is abandoned here, so nothing will pass it back in as `previous`.
            #[expect(
                unsafe_code,
                reason = "leaving the enumeration early means freeing the context this call owns, \
                          which the enumeration would otherwise have done on the next step"
            )]
            unsafe {
                CertFreeCertificateContext(context);
            }

            return Ok(true);
        }

        previous = context;
    }
}

/// Put `der` into `LocalMachine\Root`.
///
/// **`CERT_STORE_ADD_NEW`, not `REPLACE_EXISTING`**: a certificate already there is the answer
/// `Change::Unchanged`, and replacing one would rewrite whatever else it carries — a store entry
/// holds friendly names and trust properties beside the encoded certificate, and none of those are
/// ours to overwrite.
///
/// # Errors
///
/// [`Error::Os`](crate::Error::Os) when the store cannot be opened for writing, which on this system
/// is what an unelevated caller gets, or when the add itself is refused.
#[cfg(feature = "elevated")]
pub(crate) fn add(der: &[u8]) -> crate::Result<bool> {
    use windows_sys::Win32::Foundation::CRYPT_E_EXISTS;
    use windows_sys::Win32::Security::Cryptography::{
        CERT_STORE_ADD_NEW, CertAddEncodedCertificateToStore,
    };

    let store = open(false)?;

    // # Safety
    //
    // `store.0` is open for the call. `der` is a live slice and its length is its own; the store
    // copies the bytes, so nothing here has to outlive the call. A null `ppCertContext` asks for no
    // context back, which is what keeps this from having one more thing to free.
    #[expect(
        unsafe_code,
        reason = "adding to a certificate store has no safe equivalent, and `certutil.exe` was \
                  refused in the T49a design's D6"
    )]
    let added = unsafe {
        CertAddEncodedCertificateToStore(
            store.0,
            X509_ASN_ENCODING,
            der.as_ptr(),
            u32::try_from(der.len()).unwrap_or(u32::MAX),
            CERT_STORE_ADD_NEW,
            std::ptr::null_mut(),
        )
    };

    if added != 0 {
        return Ok(true);
    }

    let failure = std::io::Error::last_os_error();

    // Already there. Not a failure and not a change — the answer every whole-state operation in this
    // workspace gives when the machine already says what was asked for.
    //
    // **`CRYPT_E_EXISTS` and not `ERROR_ALREADY_EXISTS`, which is what this said until a real store
    // answered.** `CertAddEncodedCertificateToStore` reports through `SetLastError`, but what it
    // sets is the crypto layer's `HRESULT` rather than a Win32 code, and the two do not overlap:
    // 0x80092005 against 183. The wrong constant cost nothing on the first install and turned the
    // second into `Failed`, which is why the round trip in `mixengine-elevate/tests/system.rs` asks
    // for the second answer as well as the first.
    if failure.raw_os_error() == Some(CRYPT_E_EXISTS) {
        return Ok(false);
    }

    Err(crate::Error::Os {
        action: "add MixEngine's certificate authority to this machine's trusted roots",
        source: failure,
    })
}

/// Remove every certificate in `LocalMachine\Root` that `ours` accepts.
///
/// **`ours` runs against what was found, not against what was asked for**, which is the second of
/// D5's two checks: a request names an authority by key-id, and this is what makes sure the thing
/// carrying that name is also shaped like one MixEngine made.
///
/// # Errors
///
/// As [`add`], and when a delete is refused.
#[cfg(feature = "elevated")]
pub(crate) fn remove(mut ours: impl FnMut(&[u8]) -> bool) -> crate::Result<usize> {
    use windows_sys::Win32::Security::Cryptography::{
        CertDeleteCertificateFromStore, CertDuplicateCertificateContext,
    };

    let store = open(false)?;
    let mut previous: *mut CERT_CONTEXT = std::ptr::null_mut();
    let mut removed = 0;

    loop {
        // # Safety
        //
        // As `each_certificate`: the enumeration frees the previous context itself, so `previous` is
        // handed over rather than freed here.
        #[expect(
            unsafe_code,
            reason = "walking the store is how a removal finds what it was asked to remove"
        )]
        let context = unsafe { CertEnumCertificatesInStore(store.0, previous) };

        if context.is_null() {
            return Ok(removed);
        }

        // # Safety
        //
        // `context` is live and its two fields describe a buffer the store keeps alive until the
        // context is freed. The slice does not outlive this iteration.
        #[expect(
            unsafe_code,
            reason = "the encoded certificate is a pointer and a length, and D5's second check runs \
                      over exactly those bytes"
        )]
        let found = unsafe {
            std::slice::from_raw_parts((*context).pbCertEncoded, (*context).cbCertEncoded as usize)
        };

        if !ours(found) {
            previous = context;
            continue;
        }

        // **The duplicate is what keeps the enumeration alive.** `CertDeleteCertificateFromStore`
        // frees the context it is given, so deleting the one the walk is standing on would leave
        // `previous` dangling for the next step. The duplicate is deleted and the original carries
        // the walk forward.
        //
        // # Safety
        //
        // `context` is live; `CertDuplicateCertificateContext` takes a reference to it and returns a
        // second one, and the delete below consumes exactly that second one.
        #[expect(
            unsafe_code,
            reason = "deleting the context the enumeration is standing on would free the pointer \
                      the next step is about to be handed"
        )]
        let deleted = unsafe {
            let copy = CertDuplicateCertificateContext(context);
            CertDeleteCertificateFromStore(copy)
        };

        if deleted == 0 {
            return Err(crate::Error::Os {
                action: "remove MixEngine's certificate authority from this machine's trusted roots",
                source: std::io::Error::last_os_error(),
            });
        }

        removed += 1;
        previous = context;
    }
}
