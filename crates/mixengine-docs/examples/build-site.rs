//! The command line of the documentation site generator — roadmap task T90.
//!
//! Five lines, deliberately: everything it does is in `support/generate.rs`, which
//! `crates/mixengine-docs/tests/site.rs` includes the same way this file does. An example rather
//! than a `[[bin]]` so that the Markdown renderer stays a dev-dependency and is linked into nothing
//! that ships.
//!
//! Run it through `packaging/docs.sh` rather than directly.

use std::path::PathBuf;

#[path = "support/generate.rs"]
mod generate;

fn main() {
    let out = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("target/site"), PathBuf::from);

    generate::build(&out);
    println!("{}", out.display());
}
