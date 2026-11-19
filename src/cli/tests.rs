use std::ffi::OsString;
use std::path::PathBuf;

use super::*;
use crate::system::Fake;

fn argv(words: &[&str]) -> Vec<OsString> {
    std::iter::once(PROGRAM)
        .chain(words.iter().copied())
        .map(OsString::from)
        .collect()
}

#[test]
fn test_the_definition_is_consistent() {
    definition().debug_assert();
}

#[test]
fn test_a_repeated_option_keeps_its_last_value() {
    let Ok(Command::Disk(args)) = parse(argv(&["disk", "--format", "json", "--format", "text"]))
    else {
        panic!("disk parses");
    };
    assert_eq!(args.format.format, Format::Text);
    let Ok(Command::Archive(commands::archive::Command::Write(args))) = parse(argv(&[
        "archive",
        "write",
        "--repository",
        "a",
        "--repository",
        "b",
        "--dry-run",
        "--dry-run",
    ])) else {
        panic!("archive write parses");
    };
    assert_eq!(args.repository.repository, Some(PathBuf::from("b")));
    assert!(args.dry_run.dry_run);
}

#[test]
fn test_bad_usage_is_an_error() {
    for words in [
        &["status", "--apply"][..],
        &["sweep", "--fetch"],
        &["finish", "wt", "--abandon"],
        &["finish", "wt", "--reason", "x"],
        &["status", "--root", "/src"],
        &["status", "--repository", "a", "--repositories", "r.json"],
        &["status", "--format", "yaml"],
        &["disk", "--format", "markdown"],
        &["env", "--shell", "zsh"],
        &["clean"],
        &["archive"],
        &["hook", "post-commit", "x"],
        &["sessions"],
        &["log"],
        &["verdict", "x"],
        &["status", "--dry"],
    ] {
        assert!(parse(argv(words)).is_err(), "{words:?}");
    }
}

#[test]
fn test_every_command_parses_with_its_options() {
    for words in [
        &["status", "fix", "main", "--size", "--format", "markdown"][..],
        &[
            "status",
            "--repositories",
            "r.json",
            "--root",
            "/src",
            "--default-branch",
            "trunk",
        ],
        &["sweep", "--unattended", "--dry-run", "--format", "json"],
        &[
            "finish",
            "wt",
            "--abandon",
            "--reason",
            "superseded",
            "--dry-run",
        ],
        &["archive", "write", "a", "b", "--dry-run"],
        &["archive", "verify", "--repositories", "r.json"],
        &["archive", "list", "fix"],
        &["archive", "restore", "fix", "--at", "2026-11-19T09:12:00Z"],
        &["clean", "wt", "--dry-run", "--format", "json"],
        &["env", "--languages", "rust,python", "--shell", "fish"],
        &["provision"],
        &["provision", "wt"],
        &["disk", "--format", "json"],
        &["rules"],
        &["facts", "--repository", "app", "--size"],
        &["hook", "reference-transaction", "committed"],
    ] {
        assert!(
            parse(argv(words)).is_ok(),
            "{words:?}: {:?}",
            parse(argv(words)).err()
        );
    }
}

#[test]
fn test_every_command_has_an_example() {
    let named: std::collections::BTreeSet<_> =
        commands::EXAMPLES.iter().map(|(_, name)| *name).collect();
    assert_eq!(
        named,
        crate::model::ids::CommandName::ALL
            .iter()
            .copied()
            .collect()
    );
    for (words, _) in commands::EXAMPLES {
        assert!(parse(argv(words)).is_ok(), "{words:?}");
    }
}

#[test]
fn test_help_and_version_print_and_succeed() {
    let mut system = Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"));
    assert_eq!(run(argv(&["--version"]), &mut system), Status::Done);
    assert_eq!(
        system.stdout,
        format!("{PROGRAM} {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(run(argv(&["sweep", "--help"]), &mut system), Status::Done);
    assert!(system.stdout.contains("--unattended"));
    assert!(
        system
            .stdout
            .contains(&crate::names::SettingKey::Unattended.config_key())
    );
}

#[test]
fn test_every_option_is_long() {
    fn shorts(command: &clap::Command) -> Vec<String> {
        command
            .get_arguments()
            .filter_map(|arg| {
                arg.get_short()
                    .map(|short| format!("{} -{short}", command.get_name()))
            })
            .chain(command.get_subcommands().flat_map(shorts))
            .collect()
    }
    let mut definition = definition();
    definition.build();
    assert_eq!(shorts(&definition), Vec::<String>::new());
}

#[test]
fn test_each_command_knows_its_name_and_format() {
    for (words, name) in commands::EXAMPLES {
        let command = parse(argv(words)).expect("parses");
        assert_eq!(command.name(), name, "{words:?}");
        assert!(!command.json(), "{words:?}");
    }
    let json = |words: &[&str]| parse(argv(words)).expect("parses").json();
    assert!(json(&["status", "--format", "json"]));
    assert!(json(&["archive", "list", "--format", "json"]));
    assert!(!json(&["status", "--format", "markdown"]));
}
