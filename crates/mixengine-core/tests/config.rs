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
    // install with it. `.` says the same thing in a different way.
    for empty in ["", ".", "./"] {
        let home = TempDir::new().unwrap();
        let path = write(&home, &format!("[paths]\ndata = \"{empty}\"\n"));

        let error = config::load(&path).unwrap_err();

        assert!(
            matches!(error, mixengine_core::Error::Config { .. }),
            "data = {empty:?} was accepted: {error:?}"
        );
        assert!(reported(&error).contains("data"), "{error}");
    }
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
