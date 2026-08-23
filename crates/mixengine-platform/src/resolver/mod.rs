//! Routing a managed TLD to MixEngine's own DNS server — roadmap task **T45**.
//!
//! T44 built a server that answers `A 127.0.0.1` for every name under a managed TLD, and nothing
//! that sends it a query. This is the other half: three mechanisms, one per system, none of them
//! interchangeable — a file per TLD on macOS, a network link of our own on Linux, one registry rule
//! on Windows.
//!
//! **Each system's text or values are generated in a submodule here, pure and compiled
//! everywhere**, exactly as [`crate::port_access`] does it. That is what lets a developer on any one
//! of the three test all three; only the writes live in `crate::sys::resolver`.
//!
//! Compiled under **both** `host` and `elevated`, for [`crate::hosts`]' reason: the daemon reads the
//! state and the helper writes it, and neither is worth a second implementation.

// Every one of these has its consumer two commits away, in `crate::sys::resolver`, and until then
// only their own tests call them. `expect` rather than `allow` on purpose: the day the per-OS
// modules land, this attribute becomes an unfulfilled expectation and the compiler asks for it back
// — which `allow` would not. Under `cfg_attr(not(test))` because in a test build they *are* used,
// so the lint does not fire there and an unconditional `expect` would be unfulfilled at once.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the per-OS writers that call these land in the next three commits"
    )
)]
pub(crate) mod directory;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the per-OS writers that call these land in the next three commits"
    )
)]
pub(crate) mod networkd;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the per-OS writers that call these land in the next three commits"
    )
)]
pub(crate) mod nrpt;
