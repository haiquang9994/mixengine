//! Generate `extensions.json` for the packaging repository to sign — roadmap task **T81a**.
//!
//! ```text
//! cargo run -p mixengine-core --example extensions_json -- \
//!     --manifests data/extensions \
//!     --pub minisign.pub \
//!     --generated-at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
//!     --out dist/extensions.json
//! ```
//!
//! Everything that decides anything is `registry::assemble`; this is argument parsing, one file
//! read and one file write. It is an *example* rather than a binary because `mix` may not depend on
//! this crate (`mixengine-proto/tests/workspace_layering.rs` is the test that keeps it so), and a
//! workspace member that ships nothing would be a fourth thing in the layout list — while an
//! example is already built by `cargo clippy --all-targets`, so it cannot rot without CI saying so.
//!
//! `--generated-at` is passed in rather than read off a clock because this workspace has no date
//! library; `index::format::Timestamp` records why. The shell has `date -u`.

use std::path::PathBuf;
use std::process::ExitCode;

use mixengine_core::extensions::registry;
use mixengine_core::index::Timestamp;

/// What the four flags were given.
struct Arguments {
    manifests: PathBuf,
    public_key: PathBuf,
    generated_at: Timestamp,
    out: PathBuf,
}

const USAGE: &str = "\
usage: extensions_json --manifests <dir> --pub <file> --generated-at <stamp> --out <file>

  --manifests     data/extensions in a mixengine-packages checkout
  --pub           the minisign.pub committed beside it
  --generated-at  when this run started, as date -u +%Y-%m-%dT%H:%M:%SZ
  --out           where to write the document";

fn main() -> ExitCode {
    let arguments = match parse() {
        Ok(arguments) => arguments,
        Err(complaint) => {
            eprintln!("{complaint}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(&arguments) {
        Ok(count) => {
            println!(
                "wrote {} with {count} extension(s), generated at {}",
                arguments.out.display(),
                arguments.generated_at
            );
            ExitCode::SUCCESS
        }
        Err(reason) => {
            // The chain, not only its head: the manifest reader's failures carry the TOML line and
            // column as a `source`, and printing the head alone drops the part that says where.
            eprintln!("error: {reason}");
            let mut next = std::error::Error::source(reason.as_ref());
            while let Some(cause) = next {
                eprintln!("  caused by: {cause}");
                next = cause.source();
            }
            ExitCode::FAILURE
        }
    }
}

/// Assemble, serialise, write. The count is what the caller prints.
///
/// **Boxed rather than returned bare.** `mixengine_core::Error` is over 128 bytes, so carrying it
/// up through this frame as well as through `assemble`'s is what `clippy::result_large_err` is
/// about — the same lint `mixengine-daemon`'s certificate module answers by converting at its
/// boundary. There is no wire error to convert to here, so the box is the boundary.
fn run(arguments: &Arguments) -> Result<usize, Box<mixengine_core::Error>> {
    let registry = registry::assemble(
        &arguments.manifests,
        &arguments.public_key,
        arguments.generated_at,
    )
    .map_err(Box::new)?;

    let mut document =
        serde_json::to_string_pretty(&registry).expect("a registry is made of strings and maps");
    document.push('\n');

    std::fs::write(&arguments.out, document).map_err(|source| {
        Box::new(mixengine_core::Error::Io {
            action: "write",
            path: arguments.out.clone(),
            source,
        })
    })?;

    Ok(registry.extensions.len())
}

/// Every flag takes a value and there are no positional arguments, which is what lets this be
/// twenty lines rather than a dependency `mixengine-core` does not otherwise have.
fn parse() -> Result<Arguments, String> {
    let mut manifests = None;
    let mut public_key = None;
    let mut generated_at = None;
    let mut out = None;

    let mut argv = std::env::args().skip(1);
    while let Some(flag) = argv.next() {
        let value = argv.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--manifests" => manifests = Some(PathBuf::from(value)),
            "--pub" => public_key = Some(PathBuf::from(value)),
            "--generated-at" => generated_at = Some(value),
            "--out" => out = Some(PathBuf::from(value)),
            _ => return Err(format!("unknown argument {flag}")),
        }
    }

    let generated_at = generated_at.ok_or("--generated-at is required")?;

    Ok(Arguments {
        manifests: manifests.ok_or("--manifests is required")?,
        public_key: public_key.ok_or("--pub is required")?,
        generated_at: generated_at
            .parse()
            .map_err(|reason: mixengine_core::Error| reason.to_string())?,
        out: out.ok_or("--out is required")?,
    })
}
