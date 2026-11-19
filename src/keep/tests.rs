use std::path::PathBuf;

use super::*;

fn file(path: &str) -> Ignored {
    Ignored {
        path: path.into(),
        kind: IgnoredKind::File,
    }
}

fn repository(path: &str) -> Ignored {
    Ignored {
        path: path.into(),
        kind: IgnoredKind::Repository,
    }
}

fn built_in() -> Patterns {
    with_file(".worktree-keep", "")
}

fn with_file(name: &str, text: &str) -> Patterns {
    patterns(&[(name.into(), text.as_bytes().to_vec())], false).expect("patterns")
}

fn paths(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

/// Each built-in pattern, with a path it keeps.
const KEPT_SAMPLES: [(&str, &str); 20] = [
    (".env", ".env"),
    (".env.*", ".env.production"),
    (".envrc", ".envrc"),
    (".envrc.local", ".envrc.local"),
    ("*.local.*", "config.local.toml"),
    (".claude/settings.local.json", ".claude/settings.local.json"),
    (".idea/", ".idea/workspace.xml"),
    ("*.iml", "app.iml"),
    (".superpowers/", ".superpowers/plan.md"),
    ("HANDOFF.md", "HANDOFF.md"),
    ("*.tfplan", "infra.tfplan"),
    ("tfplan*", "state/tfplan"),
    ("*.tfstate*", "terraform.tfstate.backup"),
    (".xtdb/", ".xtdb/db"),
    (".nrepl-port", ".nrepl-port"),
    ("*-workspace/", "skills/x-workspace/out.json"),
    (".gstack/", ".gstack/session"),
    ("reports/", "reports/r.txt"),
    (".worktree-keep", ".worktree-keep"),
    (".worktreeinclude", ".worktreeinclude"),
];

#[test]
fn test_every_built_in_pattern_keeps_its_sample() {
    assert_eq!(KEPT_SAMPLES.map(|(pattern, _)| pattern), BUILT_IN);
    let patterns = built_in();
    for (pattern, sample) in KEPT_SAMPLES {
        assert!(
            is_kept(&patterns, &file(sample)),
            "{pattern} keeps {sample}"
        );
    }
}

#[test]
fn test_build_output_is_what_no_pattern_keeps() {
    let patterns = built_in();
    for path in [
        "target/debug/app",
        "build/out.o",
        ".claude/other.json",
        "envrc",
        "reports",
        "a.local",
    ] {
        assert!(!is_kept(&patterns, &file(path)), "{path}");
    }
}

#[test]
fn test_a_name_pattern_matches_at_any_depth_and_an_anchored_one_from_the_root() {
    let patterns = with_file(".worktree-keep", "/top.txt\nnotes/*.md\n");
    assert!(is_kept(&patterns, &file("deep/in/.env")));
    assert!(is_kept(&patterns, &file("top.txt")));
    assert!(!is_kept(&patterns, &file("sub/top.txt")));
    assert!(is_kept(&patterns, &file("notes/a.md")));
    assert!(!is_kept(&patterns, &file("other/notes/a.md")));
}

#[test]
fn test_a_later_negation_undoes_an_earlier_pattern() {
    let patterns = with_file(".worktreeinclude", "!.env.example\n");
    assert!(!is_kept(&patterns, &file(".env.example")));
    assert!(is_kept(&patterns, &file(".env.production")));
}

#[test]
fn test_patterns_fold_case_where_the_repository_ignores_it() {
    let files = [(PathBuf::from(".worktree-keep"), b"Notes/\n".to_vec())];
    let sensitive = patterns(&files, false).expect("patterns");
    let folding = patterns(&files, true).expect("patterns");
    for path in ["handoff.md", "notes/a.md", ".ENV"] {
        assert!(!is_kept(&sensitive, &file(path)), "{path}");
        assert!(is_kept(&folding, &file(path)), "{path}");
    }
    assert!(is_kept(&sensitive, &file("HANDOFF.md")));
}

#[test]
fn test_split() {
    let entries = [
        file(".env"),
        file(".envrc"),
        file("target/debug/app"),
        repository("vendor/lib"),
        file(".idea/workspace.xml"),
    ];
    let same = |entry: &Ignored| entry.path.as_path() == std::path::Path::new(".envrc");
    assert_eq!(
        split(&entries, &built_in(), same),
        Split {
            kept: paths(&[".env", ".envrc", "vendor/lib", ".idea/workspace.xml"]),
            own: paths(&[".env", "vendor/lib", ".idea/workspace.xml"]),
            build_output: paths(&["target/debug/app"]),
        }
    );
}

#[test]
fn test_a_nested_repository_is_always_its_own() {
    let split = split(&[repository("vendor/lib")], &built_in(), |_| true);
    assert_eq!(split.own, paths(&["vendor/lib"]));
    assert!(split.build_output.is_empty());
}

#[test]
fn test_read_patterns_takes_each_file_from_the_worktree_or_else_the_main_checkout() {
    use crate::system::Fake;
    let system = Fake::new("2026-11-19T09:12:00Z".parse().expect("a time"))
        .with_file("/main/.worktree-keep", "from-main\n")
        .with_file("/main/.worktreeinclude", "main-include\n")
        .with_file("/wt/.worktreeinclude", "wt-include\n");
    let patterns = read_patterns(
        &system,
        std::path::Path::new("/wt"),
        Some(std::path::Path::new("/main")),
        false,
    )
    .expect("readable");
    assert!(is_kept(&patterns, &file("from-main")));
    assert!(is_kept(&patterns, &file("wt-include")));
    assert!(!is_kept(&patterns, &file("main-include")));
}
