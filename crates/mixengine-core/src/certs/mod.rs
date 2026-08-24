//! Certificates: the authority this home signs with, and later the leaves it signs.
//!
//! Domain logic only, as everything in this crate is. Writing the private key with permissions
//! nobody else can read through is [`mixengine_platform::write_private`], because a file mode is an
//! operating system's business and this crate makes no OS calls.

pub mod ca;
