//! Reading `config.toml`, and keeping the shipped template honest.

use std::path::PathBuf;

use mixengine_core::config::{
    self, Config, Daemon, LogFormat, LogLevel, Logging, PathOverrides, TEMPLATE,
};
use tempfile::TempDir;

fn write(home: &TempDir, contents: &str) -> PathBuf {
    let path = home.path().join(config::FILE_NAME);
    std::fs::write(&path, contents).expect("the temporary home is writable");
    path
}

/// What a user actually sees: the error and everything under it.
///
/// The parse failure is the `#[source]` rather than part of the top-level message, so a test that
/// only looked at `to_string()` would miss the half that says which key is wrong.
fn reported(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        message.push('\n');
        message.push_str(&next.to_string());
        cause = next.source();
    }
    message
}

#[test]
fn a_missing_file_means_defaults() {
    let home = TempDir::new().unwrap();

    let config = config::load(&home.path().join(config::FILE_NAME)).unwrap();

    assert_eq!(config, Config::default());
    assert_eq!(config.log.level, LogLevel::Info);
    assert_eq!(config.log.format, LogFormat::Text);
    assert_eq!(config.daemon.ipc_path, None);
    assert_eq!(config.daemon.shutdown_grace_seconds, 10);
}

#[test]
fn a_shutdown_grace_of_zero_is_a_setting_and_not_an_absent_key() {
    // The one value a derived `Default` would have produced, which is why `Daemon` writes its own:
    // "kill everything at once" is a choice somebody can make, and it must not be what a user gets
    // for leaving the key out.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[daemon]\nshutdown_grace_seconds = 0\n");

    let config = config::load(&path).unwrap();

    assert_eq!(config.daemon.shutdown_grace_seconds, 0);
    assert_ne!(
        config.daemon.shutdown_grace_seconds,
        Daemon::default().shutdown_grace_seconds
    );
}

#[test]
fn a_shutdown_grace_past_the_ceiling_is_refused_rather_than_lowered() {
    // One second over, so what is being checked is the boundary and not "a big number looks wrong".
    // The message has to carry both halves: the number that was refused, because a config file long
    // enough to hold a mistake is long enough that "somewhere in here" is not an answer, and the
    // number that would be accepted, because a ceiling nobody can read is a ceiling nobody can
    // satisfy.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[daemon]\nshutdown_grace_seconds = 601\n");

    let error = config::load(&path).unwrap_err();
    let message = reported(&error);

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
    assert!(message.contains("601"), "{message}");
    assert!(message.contains("600"), "{message}");
}

#[test]
fn a_shutdown_grace_of_u64_max_never_reaches_the_arithmetic_that_would_panic() {
    // The value from the report, and the reason there is a ceiling at all: the daemon turns this
    // into a `Duration` and adds it to an `Instant`, and that addition panics on overflow rather
    // than saturating — on the shutdown path, unwinding past the WAL checkpoint that a clean stop
    // exists to perform. Asserting the ceiling is named in the message also pins the claim about
    // `toml` this all rests on: TOML integers are decoded into whatever width holds them, so
    // `u64::MAX` arrives here intact rather than being refused earlier as too large for an `i64`.
    let home = TempDir::new().unwrap();
    let path = write(
        &home,
        &format!("[daemon]\nshutdown_grace_seconds = {}\n", u64::MAX),
    );

    let error = config::load(&path).unwrap_err();
    let message = reported(&error);

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
    assert!(message.contains("18446744073709551615"), "{message}");
    assert!(message.contains("600"), "{message}");
}

#[test]
fn a_shutdown_grace_under_the_ceiling_arrives_exactly_as_written() {
    // Including the ceiling itself: the bound is "no more than", and a user who reads the template
    // and types the largest number it names must not be told it is too large. 300 is the ordinary
    // case — a value somebody would plausibly set for a slow database, well clear of the bound and
    // untouched by it.
    for seconds in [300, 600] {
        let home = TempDir::new().unwrap();
        let path = write(
            &home,
            &format!("[daemon]\nshutdown_grace_seconds = {seconds}\n"),
        );

        let config = config::load(&path).unwrap();

        assert_eq!(config.daemon.shutdown_grace_seconds, seconds);
    }
}

#[test]
fn a_daemon_section_without_the_budget_still_gets_the_default() {
    // The section is present and the key is not, which is the one arrangement the checking of this
    // value could quietly break: a bound expressed as a field-level `default` would replace the
    // section's own, and the absent key would start meaning zero — "kill everything at once" — for
    // everybody who ever set an `ipc_path`.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[daemon]\nipc_path = \"/tmp/mixengined.sock\"\n");

    let config = config::load(&path).unwrap();

    assert_eq!(
        config.daemon.shutdown_grace_seconds,
        Daemon::default().shutdown_grace_seconds
    );
    assert_eq!(config.daemon.shutdown_grace_seconds, 10);
}

#[test]
fn first_run_writes_the_template_and_still_reads_defaults() {
    let home = TempDir::new().unwrap();
    let path = home.path().join(config::FILE_NAME);

    let config = config::load_or_create(&path).unwrap();

    assert_eq!(config, Config::default());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
}

#[test]
fn an_existing_file_is_never_overwritten() {
    let home = TempDir::new().unwrap();
    let path = write(&home, "[log]\nlevel = \"debug\"\n");

    assert!(!config::write_template(&path).unwrap());

    let config = config::load_or_create(&path).unwrap();
    assert_eq!(config.log.level, LogLevel::Debug);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[log]\nlevel = \"debug\"\n"
    );
}

#[test]
fn every_setting_round_trips() {
    // Absolute means something different on each OS, so the fixture uses whatever "a second disk"
    // looks like here — the point is that a full path survives unchanged next to a relative one.
    let bulk = if cfg!(windows) {
        r"D:\bulk"
    } else {
        "/mnt/bulk"
    };
    let home = TempDir::new().unwrap();
    let path = write(
        &home,
        &format!(
            r#"
[log]
level = "trace"
format = "json"

[daemon]
ipc_path = "/run/user/1000/mixengined.sock"
shutdown_grace_seconds = 30

[paths]
runtimes = "{bulk}/runtimes"
packages = "{bulk}/packages"
data = "{bulk}/data"
logs = "logs-elsewhere"
"#
        )
        .replace('\\', "/"),
    );

    let config = config::load(&path).unwrap();

    assert_eq!(
        config,
        Config {
            log: Logging {
                level: LogLevel::Trace,
                format: LogFormat::Json,
            },
            daemon: Daemon {
                ipc_path: Some(PathBuf::from("/run/user/1000/mixengined.sock")),
                shutdown_grace_seconds: 30,
            },
            paths: PathOverrides {
                runtimes: Some(PathBuf::from(format!("{bulk}/runtimes").replace('\\', "/"))),
                packages: Some(PathBuf::from(format!("{bulk}/packages").replace('\\', "/"))),
                data: Some(PathBuf::from(format!("{bulk}/data").replace('\\', "/"))),
                logs: Some(PathBuf::from("logs-elsewhere")),
            },
        }
    );
}

#[test]
fn a_partial_file_fills_the_rest_in_from_the_defaults() {
    let home = TempDir::new().unwrap();
    let path = write(&home, "[log]\nformat = \"json\"\n");

    let config = config::load(&path).unwrap();

    assert_eq!(config.log.format, LogFormat::Json);
    assert_eq!(config.log.level, LogLevel::Info);
    assert_eq!(config.paths, PathOverrides::default());
}

#[test]
fn an_unknown_key_is_refused_and_the_message_lists_what_is_accepted() {
    let home = TempDir::new().unwrap();
    let path = write(&home, "[log]\nlevl = \"debug\"\n");

    let error = config::load(&path).unwrap_err();
    let message = reported(&error);

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
    assert!(message.contains("levl"), "{message}");
    assert!(message.contains("level"), "{message}");
    assert!(message.contains("config.toml"), "{message}");
    // The line number matters: a config file long enough to make a typo in is long enough that
    // "somewhere in here" is not an answer.
    assert!(message.contains("line 2"), "{message}");
}

#[test]
fn an_unknown_section_is_refused_too() {
    let home = TempDir::new().unwrap();
    let path = write(&home, "[telemetry]\nenabled = true\n");

    let error = config::load(&path).unwrap_err();

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
    assert!(reported(&error).contains("telemetry"), "{error}");
}

#[test]
fn a_value_outside_the_closed_set_is_refused() {
    let home = TempDir::new().unwrap();
    let path = write(&home, "[log]\nlevel = \"verbose\"\n");

    let error = config::load(&path).unwrap_err();

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
    assert!(reported(&error).contains("verbose"), "{error}");
}

#[test]
fn a_relocation_that_names_nothing_is_refused() {
    // `Path::join("")` gives the original path back, so an empty relocation would quietly make
    // data/ *be* MIXENGINE_HOME — and a later "reset the data directory" would take the whole
    // install with it. Everything here says the same thing in a different way: `..` and `bulk/..`
    // land on the home or its parent, `/` on a whole filesystem.
    for nowhere in ["", ".", "./", "..", "../", "bulk/..", "x/../..", "/"] {
        let home = TempDir::new().unwrap();
        let path = write(&home, &format!("[paths]\ndata = \"{nowhere}\"\n"));

        let error = config::load(&path).unwrap_err();

        assert!(
            matches!(error, mixengine_core::Error::Config { .. }),
            "data = {nowhere:?} was accepted: {error:?}"
        );
        assert!(reported(&error).contains("data"), "{error}");
    }
}

#[test]
fn a_relocation_beside_the_home_is_still_allowed() {
    // The rule above is "names nothing of its own", not "never climbs": a sibling directory is an
    // ordinary place to put a second disk's worth of data, and it contains no part of the home.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[paths]\ndata = \"../mixengine-bulk\"\n");

    let config = config::load(&path).unwrap();

    assert_eq!(config.paths.data, Some(PathBuf::from("../mixengine-bulk")));
}

#[test]
fn a_relocation_that_does_not_say_which_drive_is_refused() {
    // Windows only: `C:\home\MixEngine`.join("/bulk") is `C:\bulk` — neither inside the home nor
    // where the user was pointing, and nothing in the config file hints at it. On Unix the same
    // string is plainly absolute and means what it says.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[paths]\ndata = \"/bulk\"\n");

    let loaded = config::load(&path);

    if cfg!(windows) {
        let error = loaded.unwrap_err();
        assert!(
            matches!(error, mixengine_core::Error::Config { .. }),
            "{error:?}"
        );
        assert!(reported(&error).contains("drive"), "{error}");
    } else {
        assert_eq!(
            loaded.unwrap().paths.data,
            Some(PathBuf::from("/bulk")),
            "an absolute path is the ordinary case on Unix"
        );
    }
}

#[test]
fn a_relocation_that_names_a_drive_but_not_its_root_is_refused() {
    // The mirror of the case above, and the more dangerous one: a path carrying a drive prefix
    // replaces everything it is joined to, so `C:\home\MixEngine`.join("C:bulk") is plain `C:bulk`
    // — resolved against drive C's *current directory*, which the config file never mentions.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[paths]\ndata = \"C:bulk\"\n");

    let loaded = config::load(&path);

    if cfg!(windows) {
        let error = loaded.unwrap_err();
        assert!(
            matches!(error, mixengine_core::Error::Config { .. }),
            "{error:?}"
        );
        assert!(reported(&error).contains("drive"), "{error}");
    } else {
        assert_eq!(
            loaded.unwrap().paths.data,
            Some(PathBuf::from("C:bulk")),
            "Unix has no drive prefixes — this is a directory whose name contains a colon"
        );
    }
}

#[test]
fn a_network_share_is_a_directory_like_any_other() {
    // Windows folds the whole of `\\server\share` into the path's prefix, so counting components
    // finds nothing there and the "names no directory" rule would refuse a perfectly addressable
    // place. A share root is not a drive root: it is somewhere data can actually live.
    let home = TempDir::new().unwrap();
    let path = write(&home, "[paths]\ndata = '//server/share'\n");

    let config = config::load(&path).unwrap();

    assert_eq!(config.paths.data, Some(PathBuf::from("//server/share")));
}

#[test]
fn an_empty_ipc_path_is_refused() {
    let home = TempDir::new().unwrap();
    let path = write(&home, "[daemon]\nipc_path = \"\"\n");

    let error = config::load(&path).unwrap_err();

    assert!(
        matches!(error, mixengine_core::Error::Config { .. }),
        "{error:?}"
    );
}

#[test]
fn the_template_as_shipped_changes_nothing() {
    let config: Config = toml::from_str(TEMPLATE).unwrap();

    assert_eq!(config, Config::default());
}

#[test]
fn every_key_the_template_documents_is_a_real_key() {
    // `deny_unknown_fields` turns this into a spell-checker for the template: uncomment every key
    // line and a documented key that no longer exists, or was renamed, fails to parse.
    let uncommented: String = TEMPLATE
        .lines()
        .map(|line| {
            line.strip_prefix('#')
                .filter(|rest| is_key_line(rest))
                .unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n");

    let config: Config = toml::from_str(&uncommented).unwrap_or_else(|error| {
        panic!("the template documents a key that does not exist: {error}")
    });

    // The values shown for `[log]` are claimed to be the defaults, so they have to be.
    assert_eq!(config.log, Logging::default());
    // And so is the one shown for the shutdown budget, which the template states in so many words.
    assert_eq!(
        config.daemon.shutdown_grace_seconds,
        Daemon::default().shutdown_grace_seconds
    );
    // The rest have no default to show and carry an example instead — which must still be parsed
    // as the right type, not silently ignored.
    assert!(config.daemon.ipc_path.is_some());
    assert!(config.paths.runtimes.is_some());
    assert!(config.paths.packages.is_some());
    assert!(config.paths.data.is_some());
    assert!(config.paths.logs.is_some());
}

/// A commented-out setting (`#level = "info"`), as opposed to prose (`# How much to log`).
fn is_key_line(rest: &str) -> bool {
    rest.starts_with(|character: char| character.is_ascii_lowercase()) && rest.contains(" = ")
}
