//! One job: make `cargo` notice that a migration changed.
//!
//! `sqlx::migrate!` reads `migrations/` while the macro expands, so the SQL ends up inside the
//! binary — but a proc macro cannot tell cargo what it read on stable Rust. Without the line below,
//! editing a migration leaves `mixengine-core` looking unchanged, the crate is not rebuilt, and the
//! binary keeps the previous schema while the file on disk says otherwise. That failure is
//! invisible: the tests pass, against the old SQL.
//!
//! Naming the directory rather than each file is deliberate — a migration that is *added* has to
//! trigger a rebuild too, and a file that does not exist yet cannot be listed.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
