//! Where a library error becomes the one a client sees.
//!
//! Every crate below keeps its own `thiserror` enum, shaped for the code that raises it and knowing
//! nothing about codes, hints or the wire. The translation happens here, once, at the boundary —
//! `.claude/standards/rust.md`. Three things happen in it, and none of them belong anywhere else:
//!
//! - **The chain is flattened.** A client is handed one string and has no `source()` to walk, so
//!   every cause is folded into the message before it leaves. That part is
//!   [`mixengine_proto::flatten`], because `mix` maps the handful of failures it can meet without a
//!   daemon and has to produce the same shape of message.
//! - **A code is chosen.** That is the part a program branches on, so the choice is made by
//!   somebody who can see both sides — the library variant and the published vocabulary.
//! - **A hint is written**, where the daemon knows something the library did not. The library knows
//!   that `create` returned `EACCES`; only this layer knows that MixEngine never runs as an
//!   administrator and that `[paths]` exists. Where the library message already names the way out,
//!   the hint stays `None` rather than saying it a second time in the GUI.

use std::io;
use std::path::Path;

use mixengine_proto::{Error, ErrorCode};

/// A library error, rendered for the wire.
///
/// A trait rather than `From`, because both types are foreign here and the orphan rule says so —
/// which is no loss: `to_wire` at a call site names what is happening better than `.into()` would.
pub(crate) trait ToWire {
    /// Translate into the error a client sees.
    fn to_wire(&self) -> Error;
}

impl ToWire for mixengine_core::Error {
    fn to_wire(&self) -> Error {
        use mixengine_core::{Error as Core, services::GraphError};

        match self {
            // `kind` is the RPC namespace the entity belongs to (`site`, `runtime`, …), which is
            // also the noun `mix` uses, so the hint can name the command that would have listed it.
            Core::NotFound { kind, .. } => Error::new(ErrorCode::NotFound, chain(self))
                .with_hint(format!("`mix {kind} list` shows what does exist")),

            Core::Io { path, source, .. } => io_failure(chain(self), path, source),

            Core::Config { path, .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(format!(
                    "fix {}, or delete it and MixEngine will write a fresh one with the defaults",
                    path.display()
                )),

            // Every way this fails is a file that could not be used as a database, so `io` is what
            // a client branches on — and the reason is nearly always the directory rather than the
            // file, which is what the hint says.
            Core::Database { path, .. } => {
                Error::new(ErrorCode::Io, chain(self)).with_hint(format!(
                    "{} is opened by `mixengined` and by nothing else — a home directory that has \
                     been moved, emptied or copied from another account is the usual reason it \
                     cannot be",
                    path.display()
                ))
            }

            // The hint has to say what did *not* happen, because the database being untouched is
            // the whole point of stopping here.
            Core::Backup { path, .. } => Error::new(ErrorCode::Io, chain(self)).with_hint(format!(
                "the upgrade stopped rather than migrate a database it could not copy first — \
                 make room for {} and start MixEngine again",
                path.display()
            )),

            // Our SQL, not the user's file: `internal` is the honest name for it. The hint is
            // worth writing anyway, and it is true because each migration runs in a transaction —
            // the one that failed rolled back, so the previous release still opens this database.
            Core::Migration { .. } => Error::new(ErrorCode::Internal, chain(self)).with_hint(
                "this is a bug in this release of MixEngine — the database was left as it was, \
                 and the version you upgraded from still runs against it",
            ),

            // Not a bug and not a dead end, which is why it is not `internal`: the file is from
            // another build, and the copy taken before the upgrade is the way back.
            Core::IncompatibleDatabase { path, .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self)).with_hint(format!(
                    "MixEngine copies the database aside before it migrates one — the \
                     `{}.bak-…` next to it is from before the upgrade that did this",
                    path.display()
                ))
            }

            // Split rather than given one code, because two different things arrive here: a
            // question about a service that is not there, and a set of specs that does not describe
            // a runnable system. Neither is `internal` — the second is a fault in whatever
            // *declared* those services and not in this machine, and calling it a bug of ours would
            // send the user looking anywhere except the one file they can fix.
            Core::Graph(error) => match error {
                GraphError::NoSuchService { .. } => Error::new(ErrorCode::NotFound, chain(self))
                    .with_hint("`mix service list` shows what does exist"),

                // The message names the services involved, and for a loop it writes the loop out;
                // what it cannot know is where they were written down, which is the whole of the
                // advice available.
                _ => Error::new(ErrorCode::InvalidArgument, chain(self)).with_hint(
                    "services are declared by MixEngine's own packages and by any `extension.toml` \
                     you have added — the edge to change is in whichever of them declares the \
                     services named above",
                ),
            },

            // **Reachable by a client since T23, and unclassified before it.** T20 and T21 landed
            // every variant below with no method in front of them, so each fell into the `internal`
            // arm at the bottom of this match — which was true of nothing except that nobody could
            // reach them. `runtime.*` is what made them a user's problem, so this is where they get
            // a code and a way out.
            //
            // The split that matters is *whose fault it is*, because that is what decides where a
            // person is sent: a document or an archive that verified and is then unusable is one we
            // published, and a signature or a checksum that does not match is somebody between us
            // and them. Only the first is `internal`.
            Core::IndexTransport { .. } | Core::ArtifactTransport { .. } => {
                Error::new(ErrorCode::Io, chain(self)).with_hint(
                    "MixEngine fetches the version list and every runtime over the internet — a \
                     machine that is offline, or behind a proxy that needs a certificate this one \
                     does not trust, is the usual reason it cannot",
                )
            }

            // The download stopped part way after every resume attempt, and what is on disk is
            // *kept*: asking again continues from it rather than starting over, which is the one
            // thing worth telling somebody who is about to try.
            Core::ArtifactIncomplete { .. } => Error::new(ErrorCode::Io, chain(self))
                .with_hint("what arrived is kept — asking again resumes from it"),

            // The one failure that cannot happen by accident, and the one place this daemon refuses
            // something that verified as well-formed. `precondition_failed` rather than `internal`:
            // nothing here is broken, and what has to change is which server is being asked.
            Core::IndexSignature { .. } => Error::new(ErrorCode::PreconditionFailed, chain(self))
                .with_hint(
                    "the index is signed by MixEngine and checked before it is read — a mirror \
                     serving somebody else's document, or an index published by a team using its \
                     own key, needs that key given to `mixengined --index-key`",
                ),

            // Bytes that are ours by signature and then do not match what the index says they hash
            // to, or are longer than it says they are. A corrupted transfer or a mirror serving
            // something else; either way the download has been deleted and the next step is the
            // same one.
            Core::ArtifactChecksum { .. } | Core::ArtifactTooLarge { .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self)).with_hint(
                    "the download did not match what the signed index publishes for it and has \
                     been discarded — asking again fetches it afresh",
                )
            }

            // An archive shape, or a document version, from a pipeline newer than this build. Not a
            // bug and not a dead end: the update is the fix, and saying so beats the field-by-field
            // confusion a best-effort parse would produce.
            Core::IndexSchema { .. } | Core::ArtifactFormat { .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self))
                    .with_hint("this release of MixEngine is older than what it is being offered")
            }

            // A correctly signed index from before a security release, replayed. The cached document
            // is kept, so nothing was lost — what is worth saying is that the *server* is behind.
            Core::IndexRolledBack { .. } => Error::new(ErrorCode::PreconditionFailed, chain(self))
                .with_hint(
                    "the index already held is newer and is still being used — a mirror that has \
                     stopped syncing is the usual reason a server offers an older one",
                ),

            // The archive names a path outside where it is being unpacked: the oldest attack there
            // is against an installer, and one a correct signature says nothing about. Nothing was
            // written — the staging directory it was going into is gone.
            Core::UnsafeArchiveEntry { .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self)).with_hint(
                    "nothing was unpacked — please report this, quoting the archive named above",
                )
            }

            // Past the signature and past the checksum, so these bytes *are* the ones we published
            // and they are unusable: our packaging, our bug. The same relationship
            // `IndexUnreadable` has to `IndexSignature`, one layer down.
            Core::IndexUnreadable { .. }
            | Core::ArchiveUnreadable { .. }
            | Core::MissingFromArtifact { .. } => Error::new(ErrorCode::Internal, chain(self))
                .with_hint(
                    "this is a bug in what MixEngine published rather than on this machine — \
                     `logs/daemon.log` has the detail a report needs",
                ),

            // **What a checksum cannot say.** The bytes are ours and this machine will not run them:
            // a missing VC++ redistributable, a glibc older than the build's floor, an image the
            // loader refuses. `dependency_missing` is the code that tells a GUI to offer the thing
            // that is missing, which is exactly the shape of every one of those.
            Core::SmokeTestFailed { .. } => Error::new(ErrorCode::DependencyMissing, chain(self))
                .with_hint(
                    "nothing was installed — the runtime was run once from a staging directory and \
                     would not start, so the message above is the operating system's own",
                ),

            // A broken build: the key compiled into this binary is not a key. Nothing a user did and
            // nothing they can do.
            Core::IndexKey { .. } => Error::new(ErrorCode::Internal, chain(self)),

            // `UnsupportedPlatform` and not `InvalidArgument`: the request is well formed and is
            // answerable on another machine — the same name is a module on Windows and linked in on
            // the Unix cells — so what refuses it is this build rather than the sentence.
            Core::ExtensionCompiledIn { .. } => {
                Error::new(ErrorCode::UnsupportedPlatform, chain(self)).with_hint(
                    "this build has it linked in, so it is always loaded; nothing can unload it \
                     short of a build that ships it as a module",
                )
            }

            // Two ways of saying "it is already here", and they are deliberately different variants
            // one layer down: `AlreadyInstalled` is a directory, `AlreadyRecorded` is a row. A
            // client cannot act differently on them, so they share a code and a hint.
            Core::AlreadyInstalled { .. }
            | Core::AlreadyRecorded { .. }
            | Core::PackageAlreadyRecorded { .. } => {
                Error::new(ErrorCode::AlreadyExists, chain(self)).with_hint(
                    "an installed version is never overwritten — uninstall it first if it is to be \
                     replaced",
                )
            }

            // The third way of saying "it is already here", and the one whose repair is different:
            // a service is not replaced by installing something, it is replaced by deleting it.
            Core::ServiceAlreadyDeclared { .. } => {
                Error::new(ErrorCode::AlreadyExists, chain(self)).with_hint(
                    "`mix service delete` first — deleting a service keeps its data directory",
                )
            }

            // The fourth, and the one whose repair is a different *argument* rather than a
            // different call: nothing has to be deleted, the create has to name somewhere else.
            // The message already carries the service that got there first, so the hint spends
            // itself on what to do about it.
            Core::DataDirectoryTaken { holder, .. } => {
                Error::new(ErrorCode::AlreadyExists, chain(self)).with_hint(format!(
                    "give this one a directory of its own, or `mix service delete {holder}` if it                      is the one that should go — two servers over one data directory corrupt it"
                ))
            }

            // The fifth way of saying "it is already here", and its repair is a different argument
            // rather than a different call: the message already names the project in the way, so
            // the hint spends itself on what to do about it.
            Core::ProjectRootTaken { holder, .. } => {
                Error::new(ErrorCode::AlreadyExists, chain(self)).with_hint(format!(
                    "`mix project show {holder}` is the one that has it — one directory is one                      project"
                ))
            }

            Core::ProjectNameTaken { .. } => Error::new(ErrorCode::AlreadyExists, chain(self))
                .with_hint("`mix project list` shows the names that are taken"),

            // The user's own argument, and the message already says which rule it broke.
            Core::InvalidProjectName { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(
                    "a project name is a handle: up to sixty-four characters, no path separators                      and no control characters",
                ),

            Core::InvalidDatabaseName { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(
                    "a database and its account are named like a project: lower-case letters, \
                     digits and hyphens, up to thirty-two characters",
                ),

            // `conflict` rather than `already_exists`: what is refused is not the name being taken
            // but MixEngine being unable to prove the account is its own to change — T77a's D3.
            Core::AccountNotOurs { .. } => Error::new(ErrorCode::Conflict, chain(self)).with_hint(
                "MixEngine only manages an account whose password it holds — `--user` picks another \
                 name, or drop that account on the server first",
            ),

            // **Not `unsupported`**, which means *this operating system cannot* and would be a lie
            // about the machine: every system this ships to runs Redis, and Redis is what has no
            // databases. The same distinction T77 drew for `blueprint.apply`.
            Core::NoDatabaseVocabulary { .. } => {
                Error::new(ErrorCode::InvalidArgument, chain(self)).with_hint(
                    "only the database servers have databases — `mix service list` shows what this \
                     home runs",
                )
            }

            // **T77 left these four in the catch-all below**, so a mistyped blueprint name reached a
            // client as an internal error. Found while adding the three above; fixed here rather
            // than left for whoever meets it next.
            Core::InvalidBlueprintName { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(
                    "a blueprint name is a filename stem: lower-case letters, digits and hyphens",
                ),

            Core::BlueprintExists { .. } => Error::new(ErrorCode::AlreadyExists, chain(self)),

            Core::BlueprintManifest { .. } | Core::UnknownBlueprintSchema { .. } => {
                Error::new(ErrorCode::InvalidArgument, chain(self))
            }

            Core::UnknownBlueprintSource { .. } => Error::new(ErrorCode::Internal, chain(self))
                .with_hint(
                    "this home holds a blueprint filed by a newer MixEngine than the one running",
                ),

            Core::InvalidDomain { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint("a domain is lowercase ASCII labels on .test, .localhost or .local"),

            Core::UnmanagedTld { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint("use .test — it is reserved for exactly this and resolves nowhere else"),

            Core::RiskyTld { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint("`--i-know` accepts it anyway; .test avoids the question"),

            Core::DomainTaken { .. } => Error::new(ErrorCode::AlreadyExists, chain(self))
                .with_hint("`mix site update` can move it, or pick another name"),

            Core::LastDomain { .. } => Error::new(ErrorCode::Conflict, chain(self))
                .with_hint("`mix site delete` removes the site itself"),

            Core::PrimaryDomain { .. } => Error::new(ErrorCode::Conflict, chain(self))
                .with_hint("`mix site update --domain <new-primary> --domain <the-rest>` reorders"),

            Core::DocRootOutsideProject { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint("a doc root is a directory inside the project's own root"),

            // Never reaches a client as an error: the job registry judges an ending by the token
            // rather than by what the work returned, so work that gave up when asked is recorded as
            // *cancelled*. Classified all the same, because a value that can be constructed can be
            // rendered.
            Core::InstallCancelled => Error::new(ErrorCode::PreconditionFailed, chain(self))
                .with_hint("what had been downloaded is kept — asking again resumes from it"),

            // A hand-edited database, or a row from a build that knew a channel this one does not.
            // The same reading `UnknownServiceState` gets, and the same code.
            Core::UnreadableRuntimeRow { .. }
            | Core::UnreadablePackageRow { .. }
            | Core::UnreadableProjectRow { .. }
            | Core::UnreadableSiteRow { .. } => Error::new(ErrorCode::Internal, chain(self))
                .with_hint(
                    "the row was written by a different version of MixEngine, or edited by hand",
                ),

            // The user's own file, one directory out from `Core::Config` and given the same code —
            // and the same hint would be wrong: `mixengine.toml` is checked into their repository,
            // so "delete it and a fresh one will be written" is advice about somebody else's file.
            Core::Manifest { path, .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(format!(
                    "only the `[runtimes]` table of {} is read while resolving a version — the \
                     languages it may name are php, node, python and ruby",
                    path.display()
                )),

            // The user's file again, and the same code: what a person does about either is open the
            // file. The hint differs because the repair does — nothing here is about `[runtimes]`.
            Core::ManifestEdit { path, .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(format!(
                    "{} could not be rewritten with the project in it — check that it is a TOML                      file this user can write",
                    path.display()
                )),

            // A client sent a directory that means nothing to a daemon. The message names it, and
            // what to do about it is the caller's own bug rather than the user's.
            Core::NotAbsolute { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(
                    "a directory is resolved by walking up from it, so it has to be one this \
                     machine can find on its own",
                ),

            // **The one failure `runtime.resolve` exists to produce well.** `dependency_missing` is
            // the code the feature spec names, and the hint is the whole value of the answer: what
            // to type. A range cannot become an install command — inventing a version would be
            // inventing a release — so it becomes the listing instead.
            Core::RuntimeUnresolved {
                kind, constraint, ..
            } => Error::new(ErrorCode::DependencyMissing, chain(self))
                .with_hint(mixengine_core::resolve::install_command(*kind, constraint)),

            // Nothing was asked for and there is nothing to fall back on. Distinct from the above
            // because the way out is different: there is no constraint to satisfy, only a kind with
            // no default — which is what uninstalling the last version leaves behind.
            Core::NoDefaultRuntime { kind } => {
                Error::new(ErrorCode::DependencyMissing, chain(self)).with_hint(format!(
                    "`mix runtime list --kind {kind}` shows what is installed, and \
                     `mix runtime default {kind} <version>` chooses which one is used here"
                ))
            }

            // The version is there and the program is not, which no client can produce today — the
            // shim is what looks an executable up, and it resolves in its own process without a
            // daemon. Classified anyway, on `InstallCancelled`'s reasoning: a variant that can be
            // constructed can be rendered, and `internal` would be the wrong word for the case this
            // actually covers, which is a runtime installed before its executables were recorded.
            Core::RuntimeProvidesNothing { kind, version, .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self)).with_hint(format!(
                    "`mix runtime install {kind} {version}` after uninstalling it records what the \
                     build publishes"
                ))
            }

            // **The generated-configuration family — roadmap task T30.** What separates these is the
            // same thing that separates the index failures above: whose fault it is. An override is
            // the user's, a template and a recipe are ours, and a row that cannot be read back is
            // neither — it is a database somebody edited.
            //
            // All three of these are a person's own overrides, and the message already names the
            // setting — the ones that exist, the shape it had to be, or what is wrong with the value
            // the recipe was handed. The hint says the thing the message cannot: that being refused
            // is the *feature*, because the alternative is a setting silently doing nothing.
            Core::UnknownSetting { .. } | Core::SettingType { .. } | Core::SettingValue { .. } => {
                Error::new(ErrorCode::InvalidArgument, chain(self)).with_hint(
                    "MixEngine refuses a setting it does not know rather than ignoring it, so that \
                     a typo cannot look like a setting that is in effect",
                )
            }

            // The service's own program said no. Nothing was installed — the rendering is judged in
            // a staging directory — and saying so is the whole point: a user who has just been told
            // their configuration is broken will otherwise assume their site is down.
            Core::ConfigRejected { .. } => Error::new(ErrorCode::InvalidArgument, chain(self))
                .with_hint(
                    "nothing was installed and the configuration that is live is the last one that \
                     worked — the service reading it has not been disturbed",
                ),

            // A `packages.name` this build has no recipe for: a home written by a newer MixEngine,
            // or a service an extension declared and then went away. Not `internal`, because
            // nothing is broken here and the way out is a version rather than a bug report.
            Core::NoRecipe { .. } => Error::new(ErrorCode::PreconditionFailed, chain(self))
                .with_hint(
                    "what MixEngine can run is compiled into it — this home describes a service \
                     from a newer release, or from an extension that is no longer installed",
                ),

            // Ours: a template that does not render, or a recipe that produced a spec the supervisor
            // will not take. An override can *reach* both, which is why they are not unreachable,
            // but neither is a mistake a user could have avoided.
            Core::TemplateBroken { .. } | Core::Unrunnable { .. } => {
                Error::new(ErrorCode::Internal, chain(self)).with_hint(
                    "this is a bug in MixEngine's own configuration templates rather than on this \
                     machine — `logs/daemon.log` has the detail a report needs",
                )
            }

            // A broken installation rather than anything the caller did, which is what `internal`
            // means here — but the hint is worth having anyway, because the one thing a person can
            // do about it is reinstall, and nothing in the message says so.
            Core::ShimMissing { .. } => Error::new(ErrorCode::Internal, chain(self)).with_hint(
                "a release ships mixengined and mixengine-shim in one directory — reinstall \
                 MixEngine, or build the whole workspace if this is a development tree",
            ),

            // The one a person can act on, and the reason it is not `internal` like its shim-shaped
            // sibling: nothing can be granted until this file is there, and saying so at the method
            // that needs it is what makes the sentence useful.
            Core::ElevateMissing { .. } => Error::new(ErrorCode::DependencyMissing, chain(self))
                .with_hint(
                    "a release ships mixengined and mixengine-elevate in one directory — reinstall \
                     MixEngine, or build the whole workspace if this is a development tree",
                ),

            // A caller bug: `elevation.grant` refuses an empty queue before it composes a request.
            Core::ElevateRequestEmpty => Error::new(ErrorCode::InvalidArgument, chain(self)),

            // T40a is explicit that `Completed` means the helper ran, not that it left a report, so
            // this is a state rather than an impossibility — and one nothing a user typed caused.
            Core::ElevateReportMissing { .. } => Error::new(ErrorCode::Internal, chain(self))
                .with_hint(
                    "the elevated helper ended without writing its answer — `logs/daemon.log` has \
                     the detail, and nothing was applied that it did not report",
                ),

            Core::ElevateReportUnreadable { .. } | Core::ElevateReportMismatched { .. } => {
                Error::new(ErrorCode::Internal, chain(self)).with_hint(
                    "the helper beside this daemon is not the one it expects — reinstall MixEngine",
                )
            }

            // Unreachable: a `PrivilegedOp` is one of ours and holds nothing serde can refuse.
            Core::OpUnwritable { .. } => Error::new(ErrorCode::Internal, chain(self)),

            // The message already ends in "unset it to use this platform's default location",
            // which is the entire advice available; a hint here would be the same sentence twice.
            Core::EmptyHome => Error::new(ErrorCode::InvalidArgument, chain(self)),

            // Delegated rather than flattened: the OS failure keeps its own code, and going
            // through `chain` here would print a `#[error(transparent)]` message twice.
            Core::Platform(error) => error.to_wire(),

            // `mixengine_core::Error` is `#[non_exhaustive]`, so this arm is mandatory rather than
            // chosen. A variant that lands in it is one nobody has classified yet, and `internal`
            // is the honest name for that.
            _ => Error::new(ErrorCode::Internal, chain(self)),
        }
    }
}

impl ToWire for mixengine_platform::Error {
    fn to_wire(&self) -> Error {
        use mixengine_platform::Error as Platform;

        match self {
            // `reason` is required to describe the manual workaround where there is one
            // (`.claude/architecture/platform-abstraction.md`, rule 4), and it is already in the
            // message.
            Platform::UnsupportedPlatform { .. } => {
                Error::new(ErrorCode::UnsupportedPlatform, chain(self))
            }

            // Not `io`: nothing was touched. The environment is missing something the OS considers
            // mandatory, and the user can fix that before anything else will work.
            Platform::NoHomeDirectory { .. } => {
                Error::new(ErrorCode::PreconditionFailed, chain(self))
            }

            Platform::Io { path, source, .. } => io_failure(chain(self), path, source),

            // `io` rather than `internal`, even though most ways this can happen are a bug: the
            // rest are a machine locked down past the point where a token can be read, and greeting
            // that with "report a bug" would send somebody to the wrong place. The code says the OS
            // refused something, which is true either way.
            Platform::Os { .. } => Error::new(ErrorCode::Io, chain(self)),

            // The address is computed from `MIXENGINE_HOME`, so this is the home being wrong rather
            // than anything having failed — and `reason` already ends in what to do about it.
            Platform::Address { .. } => Error::new(ErrorCode::InvalidArgument, chain(self)),

            // Not a failure of this daemon so much as a fact about the machine, which is why the
            // hint points at the daemon that *is* running instead of at something to repair.
            Platform::EndpointInUse { .. } => Error::new(ErrorCode::Conflict, chain(self))
                .with_hint(
                    "a MixEngine daemon is already running for this home — `mix status` talks to \
                     it, and `mix daemon stop` ends it",
                ),

            // A fact about the machine too, and a different one: the endpoint name carries this
            // account's own SID, so somebody else answering on it is somebody else's doing rather
            // than a second daemon of the user's. Deliberately no `mix daemon stop` in the hint —
            // there is nothing of theirs on the name to stop.
            Platform::EndpointNotOurs { .. } => Error::new(ErrorCode::Conflict, chain(self))
                .with_hint(
                    "another account created this endpoint before the daemon could — nothing has \
                     been served through it; end that process, or sign that account out, before \
                     starting MixEngine here",
                ),

            // The tool's own complaint is in the message, and it is a better hint than anything
            // that could be written here.
            Platform::Command { .. } => Error::new(ErrorCode::ProcessFailed, chain(self)),

            _ => Error::new(ErrorCode::Internal, chain(self)),
        }
    }
}

impl ToWire for mixengine_supervisor::Error {
    fn to_wire(&self) -> Error {
        use mixengine_supervisor::Error as Supervisor;

        match self {
            Supervisor::Spawn { program, source } => match source.kind() {
                // A program that is not there is not a failed process — it is a missing
                // dependency, which is the code that tells a client to offer an install.
                io::ErrorKind::NotFound => Error::new(ErrorCode::DependencyMissing, chain(self))
                    .with_hint(format!(
                        "`{program}` is neither on PATH nor where the service expects it — install \
                         the runtime that provides it, or correct its path"
                    )),

                // The file is there and the OS refused to run it, which on Unix is nearly always
                // the executable bit and never something a restart fixes.
                io::ErrorKind::PermissionDenied => {
                    Error::new(ErrorCode::ProcessFailed, chain(self))
                        .with_hint(format!("`{program}` exists but is not executable"))
                }

                _ => Error::new(ErrorCode::ProcessFailed, chain(self)),
            },

            _ => Error::new(ErrorCode::Internal, chain(self)),
        }
    }
}

impl ToWire for crate::services::Undeclarable {
    fn to_wire(&self) -> Error {
        use crate::services::Undeclarable;

        match self {
            // The user's own declaration — a cycle, a dependency naming nothing, an id used twice —
            // which `mixengine_core::Error::Graph` already maps to `invalid_argument` with the hint
            // that says where such a thing is written. Delegated rather than re-classified here, so
            // there is one answer to "a set of specs that is not a graph" and not two.
            Undeclarable::Invalid(error) => error.to_wire(),

            // **T30 gave those failures a vocabulary, and the type now carries it.** The source is
            // the daemon's own trait and could in principle be anything, but everything that
            // implements it renders through `mixengine-core` — which already knows whether a
            // failure is a typo in somebody's overrides or a bug in ours. This arm used to hold an
            // `anyhow::Error` and *downcast* to ask: it worked, and it would have stopped working
            // the first time somebody added a `.context(…)` on the way out, silently, by
            // classifying a misspelled setting as `internal` and sending its author to file a bug
            // report.
            Undeclarable::Unavailable(error) => error.to_wire(),
        }
    }
}

/// `io`, plus whatever the OS error kind implies about the way out.
///
/// Shared by `core` and `platform`, whose `Io` variants are deliberately the same shape: the path
/// belongs in the message, the OS error stays the cause, and the advice depends on which of the two
/// or three things that actually go wrong here went wrong.
fn io_failure(message: String, path: &Path, source: &io::Error) -> Error {
    let error = Error::new(ErrorCode::Io, message);

    match source.kind() {
        // The one thing a user cannot guess: MixEngine is not going to elevate its way out of this
        // one. Everything privileged is a one-shot `mixengine-elevate` call for a listed operation
        // (ADR 0005), and touching an arbitrary directory is not on the list.
        io::ErrorKind::PermissionDenied => error.with_hint(format!(
            "MixEngine runs as your own user account and never as an administrator — give \
             yourself access to {}, or move it with [paths] in config.toml",
            path.display()
        )),

        // Carefully worded: which component is missing depends on the action. `create` goes
        // through `create_dir_all`, so a failure there is the drive or the mount point rather than
        // the parent directory, while a `read` is usually the file itself. The one thing worth
        // saying is the one only this layer knows, and it holds either way.
        io::ErrorKind::NotFound => error.with_hint(
            "check the [paths] section of config.toml — a relocation onto a disk that is not \
             mounted is the usual reason a path is simply not there",
        ),

        _ => error,
    }
}

/// Flatten an error and its causes into the single string a client is given.
///
/// A local name for [`mixengine_proto::flatten`], which is where it lives because `mix` needs the
/// same one — see the note at the top of this module. Kept as a function rather than inlined at
/// every arm so that the mapping above reads the way it did when this was written here.
fn chain(error: &dyn std::error::Error) -> String {
    mixengine_proto::flatten(error)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use mixengine_core::services::GraphError;
    use mixengine_proto::ServiceId;

    use super::*;

    #[test]
    fn an_io_failure_keeps_the_path_and_says_who_is_not_going_to_fix_it() {
        let error = mixengine_core::Error::Io {
            action: "create",
            path: PathBuf::from("/srv/mixengine"),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        assert!(
            error.message.starts_with("cannot create /srv/mixengine: "),
            "the cause is appended, not dropped: {}",
            error.message
        );
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("/srv/mixengine")),
            "the hint names the path the user has to act on: {:?}",
            error.hint
        );
    }

    #[test]
    fn a_path_that_is_not_there_points_at_the_relocation_and_not_at_a_parent() {
        let error = mixengine_core::Error::Io {
            action: "create directory",
            path: PathBuf::from("Z:/mixengine/data"),
            source: io::Error::from(io::ErrorKind::NotFound),
        }
        .to_wire();

        let hint = error.hint.expect("a missing path has advice attached");
        assert!(hint.contains("[paths]"), "{hint}");
        // `create` goes through `create_dir_all`, which makes the parents itself, and the same
        // variant covers `read`, where the leaf is what is missing. Neither claim is safe to make.
        assert!(!hint.contains("directory above"), "{hint}");
    }

    #[test]
    fn a_missing_entity_is_answered_with_the_command_that_lists_them() {
        let error = mixengine_core::Error::NotFound {
            kind: "site",
            id: "blog.test".to_owned(),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(error.message, "no such site: blog.test");
        assert_eq!(
            error.hint.as_deref(),
            Some("`mix site list` shows what does exist")
        );
    }

    /// A data directory somebody else holds is the caller's mistake, not the daemon's — T36.
    ///
    /// `internal` is what the catch-all would have made of it, and `internal` tells a person to
    /// report a bug about a create they can fix by naming another directory. The hint has to name
    /// the service already there, because the repair is a choice between two directories and one of
    /// them is in use.
    #[test]
    fn a_data_directory_already_in_use_is_the_caller_s_to_fix() {
        let error = mixengine_core::Error::DataDirectoryTaken {
            path: "/srv/db".to_owned(),
            holder: "mariadb@main".to_owned(),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::AlreadyExists);
        assert_eq!(
            error.message,
            "mariadb@main already keeps its data in /srv/db"
        );
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("mariadb@main")),
            "the repair is picking another directory, and the hint says who is in this one: {:?}",
            error.hint
        );
    }

    /// The one a user can actually fix, and the one `internal` would have hidden: the loop has to
    /// survive into the message, and the code has to say whose fault it is.
    #[test]
    fn a_dependency_loop_is_the_declaration_s_fault_and_says_so() {
        let id = |value: &str| ServiceId::parse(value).expect("a valid service id");
        let error = mixengine_core::Error::Graph(GraphError::Cycle {
            path: vec![id("caddy"), id("php-fpm@8.3"), id("mariadb@main")],
        })
        .to_wire();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(
            error
                .message
                .ends_with("caddy → php-fpm@8.3 → mariadb@main → caddy"),
            "the loop is what says which edge to delete: {}",
            error.message
        );
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("extension.toml")),
            "the message names the services; the hint says where they were written: {:?}",
            error.hint
        );
    }

    /// The other half of the same variant, which is not a declaration failure at all.
    #[test]
    fn asking_about_a_service_that_is_not_there_is_a_plain_not_found() {
        let error = mixengine_core::Error::Graph(GraphError::NoSuchService {
            id: ServiceId::parse("mailpit").expect("a valid service id"),
        })
        .to_wire();

        assert_eq!(error.code, ErrorCode::NotFound);
        assert_eq!(
            error.hint.as_deref(),
            Some("`mix service list` shows what does exist")
        );
    }

    #[test]
    fn an_io_failure_with_nothing_to_suggest_carries_no_hint() {
        let error = mixengine_core::Error::Io {
            action: "read",
            path: PathBuf::from("/srv/mixengine/config.toml"),
            source: io::Error::from(io::ErrorKind::InvalidData),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        assert_eq!(error.hint, None);
    }

    #[test]
    fn a_platform_failure_keeps_its_own_code_through_core() {
        let error =
            mixengine_core::Error::Platform(mixengine_platform::Error::UnsupportedPlatform {
                capability: "PortAccess",
                reason: "no pf on this system".to_owned(),
            })
            .to_wire();

        assert_eq!(error.code, ErrorCode::UnsupportedPlatform);
        assert_eq!(
            error.message,
            "PortAccess is not available on this platform: no pf on this system"
        );
    }

    #[test]
    fn a_transparent_variant_is_not_printed_twice() {
        // What `chain` would do to `Core::Platform` if the mapping flattened it instead of
        // delegating: thiserror keeps the inner error as the source *and* borrows its message.
        let error = mixengine_core::Error::Platform(mixengine_platform::Error::NoHomeDirectory {
            reason: "%LOCALAPPDATA% is not set",
        });

        let message = chain(&error);
        assert_eq!(message, error.to_string());
        assert_eq!(message.matches("%LOCALAPPDATA%").count(), 1);
    }

    #[test]
    fn a_missing_home_directory_is_a_precondition_not_an_io_failure() {
        let error = mixengine_platform::Error::NoHomeDirectory {
            reason: "$HOME is not set",
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        // The message already ends in "set MIXENGINE_HOME to choose one explicitly".
        assert_eq!(error.hint, None);
    }

    #[test]
    fn an_endpoint_held_by_another_account_is_not_offered_as_a_daemon_to_stop() {
        // The hint for `EndpointInUse` says "`mix daemon stop` ends it", which is the wrong thing
        // to be doing when the name is somebody else's: there is no daemon of the user's on it, and
        // the advice would send them looking for one.
        let error = mixengine_platform::Error::EndpointNotOurs {
            address: r"\\.\pipe\mixengine.S-1-5-21-1-2-3-1001.6bf2c0d4e5a19837".to_owned(),
            account: "S-1-5-21-1-2-3-1002".to_owned(),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| !hint.contains("mix daemon stop")),
            "the hint sends the user after a daemon of their own: {:?}",
            error.hint
        );
    }

    #[test]
    fn an_empty_home_is_an_invalid_argument() {
        let error = mixengine_core::Error::EmptyHome.to_wire();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert_eq!(error.hint, None);
    }

    #[test]
    fn a_configuration_that_does_not_parse_names_the_file_in_its_hint() {
        let home = tempfile::tempdir().expect("a temporary directory");
        let file = home.path().join(mixengine_core::config::FILE_NAME);
        std::fs::write(&file, "[log]\nlevel = \"chatty\"\n").expect("write the broken config");

        let error = mixengine_core::config::load(&file)
            .expect_err("`chatty` is not a level")
            .to_wire();

        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("config.toml")),
            "the hint names the file to fix: {:?}",
            error.hint
        );
        // `toml` ends its complaint with a newline, which would print a blank line above the hint.
        assert_eq!(error.message.trim_end(), error.message);
        assert!(
            error.message.contains("unknown variant `chatty`"),
            "the parse failure is what makes this actionable: {}",
            error.message
        );
    }

    #[test]
    fn a_database_that_cannot_be_opened_names_the_file_and_not_the_query() {
        let error = mixengine_core::Error::Database {
            action: "open",
            path: PathBuf::from("Z:/mixengine/mixengine.db"),
            source: sqlx::Error::Io(io::Error::from(io::ErrorKind::NotFound)),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("Z:/mixengine/mixengine.db")),
            "{:?}",
            error.hint
        );
    }

    #[test]
    fn a_database_from_another_build_is_something_the_user_can_fix() {
        let error = mixengine_core::Error::IncompatibleDatabase {
            path: PathBuf::from("/home/dev/.local/share/mixengine/mixengine.db"),
            source: sqlx::migrate::MigrateError::VersionNotPresent(2),
        }
        .to_wire();

        // Not `internal`: nothing is broken, the file is simply newer than this binary.
        assert_eq!(error.code, ErrorCode::PreconditionFailed);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains(".bak")),
            "the way back is the copy taken before the upgrade: {:?}",
            error.hint
        );
    }

    #[test]
    fn a_migration_that_fails_is_ours_and_says_the_database_is_untouched() {
        let error = mixengine_core::Error::Migration {
            path: PathBuf::from("/home/dev/mixengine.db"),
            source: sqlx::migrate::MigrateError::VersionMissing(1),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Internal);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("left as it was")),
            "{:?}",
            error.hint
        );
    }

    #[test]
    fn a_backup_that_cannot_be_written_stops_the_upgrade_and_says_so() {
        let error = mixengine_core::Error::Backup {
            path: PathBuf::from("/home/dev/mixengine.db.bak-0.1.0"),
            source: sqlx::Error::Io(io::Error::from(io::ErrorKind::StorageFull)),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::Io);
        let hint = error.hint.expect("a failed backup has advice attached");
        assert!(hint.contains("mixengine.db.bak-0.1.0"), "{hint}");
        // The database is untouched, and the user needs to know that before they panic.
        assert!(hint.contains("stopped rather than migrate"), "{hint}");
    }

    #[test]
    fn a_program_that_is_not_installed_is_a_missing_dependency() {
        let error = mixengine_supervisor::Error::Spawn {
            program: "php-fpm".to_owned(),
            source: io::Error::from(io::ErrorKind::NotFound),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::DependencyMissing);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("php-fpm")),
            "the hint names the program: {:?}",
            error.hint
        );
    }

    #[test]
    fn a_program_that_will_not_run_is_a_failed_process() {
        let error = mixengine_supervisor::Error::Spawn {
            program: "php-fpm".to_owned(),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        }
        .to_wire();

        assert_eq!(error.code, ErrorCode::ProcessFailed);
    }
}
