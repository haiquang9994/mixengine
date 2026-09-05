//! `mix docs` — the handbook, offline.
//!
//! Every test here runs with **no daemon and no home**, which is the whole point of the command:
//! help is most wanted when the thing it describes will not start. Roadmap task T90.

use std::process::Command;

/// `mix`, with every variable that could reach a daemon or choose a language taken away.
fn mix() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mix"));
    command.env_remove("MIXENGINE_HOME");
    command.env_remove("MIXENGINE_LANG");
    command.env_remove("LC_ALL");
    command.env_remove("LC_MESSAGES");
    command.env_remove("LANG");
    command
}

#[test]
fn a_topic_prints_the_page_and_needs_no_daemon() {
    let output = mix().args(["docs", "index"]).output().expect("mix runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("utf-8");
    assert!(text.starts_with("# MixEngine\n"), "{text}");
    assert!(
        text.contains("https://mixnz.github.io/mixengine/en/index/"),
        "{text}"
    );
}

#[test]
fn an_english_page_names_the_vietnamese_one_in_vietnamese() {
    let output = mix().args(["docs", "index"]).output().expect("mix runs");
    let text = String::from_utf8(output.stdout).expect("utf-8");
    assert!(
        text.contains("Tiếng Việt: mix docs index --lang vi"),
        "{text}"
    );
}

#[test]
fn no_topic_lists_them() {
    let output = mix().arg("docs").output().expect("mix runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("utf-8");
    assert!(text.contains("index"), "{text}");
    assert!(text.contains("mix docs <topic>"), "{text}");
}

#[test]
fn an_unknown_topic_names_the_ones_that_exist() {
    let output = mix().args(["docs", "nonesuch"]).output().expect("mix runs");
    assert!(!output.status.success());
    let text = String::from_utf8(output.stderr).expect("utf-8");
    assert!(text.contains("nonesuch"), "{text}");
    assert!(text.contains("index"), "{text}");
}

#[test]
fn the_language_comes_from_the_flag_then_the_environment() {
    let flagged = mix()
        .args(["docs", "index", "--lang", "vi"])
        .output()
        .expect("mix runs");
    let flagged = String::from_utf8(flagged.stdout).expect("utf-8");
    assert!(flagged.contains("/vi/index/"), "{flagged}");

    let named = mix()
        .env("MIXENGINE_LANG", "vi")
        .args(["docs", "index"])
        .output()
        .expect("mix runs");
    assert!(
        String::from_utf8(named.stdout)
            .expect("utf-8")
            .contains("/vi/index/")
    );

    let posix = mix()
        .env("LANG", "vi_VN.UTF-8")
        .args(["docs", "index"])
        .output()
        .expect("mix runs");
    assert!(
        String::from_utf8(posix.stdout)
            .expect("utf-8")
            .contains("/vi/index/")
    );

    // An unknown language is English rather than an error: a command whose whole job is to explain
    // things should answer.
    let unknown = mix()
        .env("LANG", "de_DE.UTF-8")
        .args(["docs", "index"])
        .output()
        .expect("mix runs");
    assert!(
        String::from_utf8(unknown.stdout)
            .expect("utf-8")
            .contains("/en/index/")
    );
}

#[test]
fn json_carries_the_same_body() {
    let output = mix()
        .args(["docs", "index", "--json"])
        .output()
        .expect("mix runs");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert_eq!(value["topic"], "index");
    assert_eq!(value["locale"], "en");
    assert_eq!(value["title"], "MixEngine");
    assert_eq!(value["url"], "https://mixnz.github.io/mixengine/en/index/");
    assert!(
        value["body"]
            .as_str()
            .expect("a string")
            .starts_with("# MixEngine\n")
    );
}

#[test]
fn the_reference_is_a_complete_document() {
    let output = mix()
        .args(["docs", "--reference"])
        .output()
        .expect("mix runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("utf-8");

    assert!(
        text.starts_with("+++\n"),
        "the reference carries its own front matter"
    );
    assert!(text.contains("slug = \"cli\"\n"), "{text}");
    assert!(text.contains("\n# Command reference\n"), "{text}");
    for heading in [
        "## mix status",
        "## mix site",
        "### mix site create",
        "### mix runtime install",
    ] {
        assert!(
            text.contains(&format!("\n{heading}\n")),
            "missing {heading}"
        );
    }
}

#[test]
fn the_reference_is_generated_from_the_clap_tree_and_not_from_the_corpus() {
    // The one property that keeps this from being circular: the page it produces is compiled into
    // the binary that produces it, so an output that varied with the corpus would never settle.
    // Nothing here reads a page, and this is what would notice if it started to.
    let output = mix()
        .args(["docs", "--reference"])
        .output()
        .expect("mix runs");
    let text = String::from_utf8(output.stdout).expect("utf-8");
    let index = mixengine_docs::page(mixengine_docs::Locale::En, "index").expect("the index page");
    assert!(
        !text.contains(index.summary),
        "the reference is quoting the corpus back at itself"
    );
}

#[test]
fn the_reference_is_the_committed_page() {
    let output = mix()
        .args(["docs", "--reference"])
        .output()
        .expect("mix runs");
    let generated = String::from_utf8(output.stdout).expect("utf-8");
    let committed = mixengine_docs::page(mixengine_docs::Locale::En, "cli")
        .expect("the cli page is in the corpus")
        .source();
    assert_eq!(
        generated, committed,
        "docs/guide/en/cli.md is stale — run: bash packaging/docs.sh --reference"
    );
}

#[test]
fn clap_still_owns_the_help_subcommand() {
    let output = mix().args(["help", "site"]).output().expect("mix runs");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).expect("utf-8");
    assert!(
        text.contains("Declare what is served out of a project's directory"),
        "{text}"
    );
}
