//! Tests of the pure core behind env, provision and clean: values in, values
//! out, no files. Each test has the name of its case in
//! `worktrees/tests/test_core.py`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::*;

fn env(vars: &[(&str, &str)]) -> Env {
    vars.iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn lay_at(wt: &str, main: bool) -> Layout {
    layout(
        &env(&[("HOME", "/home/u")]),
        "/w/repo/.git",
        if main {
            "/w/repo/.git"
        } else {
            "/w/repo/.git/worktrees/a"
        },
        if main { "/w/repo" } else { wt },
    )
}

fn lay() -> Layout {
    lay_at("/w/repo.worktrees/a", false)
}

fn exports(plan: &EnvPlan) -> BTreeMap<String, String> {
    plan.exports.iter().cloned().collect()
}

fn object(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => panic!("an object"),
    }
}

mod build_root {
    use super::*;

    #[test]
    fn test_root_is_keyed_by_common_dir() {
        let root = repo_build_root(&env(&[("HOME", "/home/u")]), "/w/repo/.git");
        assert_eq!(
            root,
            format!(
                "/home/u/.cache/worktree-builds/repo-{}",
                sha8("/w/repo/.git")
            )
        );
    }

    #[test]
    fn test_state_dir_is_keyed_the_same_way() {
        assert_eq!(
            repo_state_dir(&env(&[("HOME", "/home/u")]), "/w/repo/.git"),
            format!(
                "/home/u/.local/state/worktrees/repo-{}",
                sha8("/w/repo/.git")
            )
        );
        assert!(
            repo_state_dir(
                &env(&[("HOME", "/h"), ("XDG_STATE_HOME", "/s")]),
                "/w/r/.git"
            )
            .starts_with("/s/worktrees/r-")
        );
    }

    #[test]
    fn test_xdg_cache_home_wins() {
        let vars = env(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/c")]);
        assert!(repo_build_root(&vars, "/w/r/.git").starts_with("/c/worktree-builds/r-"));
    }

    #[test]
    fn test_two_clones_of_one_name_differ() {
        let vars = env(&[("HOME", "/h")]);
        assert_ne!(
            repo_build_root(&vars, "/a/public/hyperscale-rs/.git"),
            repo_build_root(&vars, "/a/schnorrific/hyperscale-rs/.git")
        );
    }

    #[test]
    fn test_bare_repository_name() {
        assert_eq!(repo_name("/srv/foo.git"), "foo");
    }

    #[test]
    fn test_wt_id() {
        assert_eq!(wt_id("/w/repo", true), "main");
        assert_eq!(
            wt_id("/w/x/feat", false),
            format!("feat-{}", sha8("/w/x/feat"))
        );
    }

    #[test]
    fn test_main_checkout_layout() {
        assert!(lay_at("/w/repo.worktrees/a", true).is_main);
        assert!(lay_at("/w/repo.worktrees/a", true).wt.ends_with("/wt/main"));
        assert!(!lay().is_main);
    }
}

mod languages {
    use super::*;

    #[test]
    fn test_detect() {
        assert_eq!(
            detect_languages(&["Cargo.toml", "pyproject.toml", "x.tf"]),
            vec![Language::Rust, Language::Python, Language::Terraform]
        );
        assert_eq!(detect_languages(&["App.xcodeproj"]), vec![Language::Xcode]);
        assert_eq!(detect_languages(&["README.md"]), Vec::new());
    }

    #[test]
    fn test_parse_with_aliases_and_unknown() {
        assert_eq!(
            parse_languages("rust, kotlin,java tofu"),
            (
                vec![Language::Rust, Language::Gradle, Language::Terraform],
                Vec::new()
            )
        );
        assert_eq!(
            parse_languages("cobol,rust"),
            (vec![Language::Rust], strings(&["cobol"]))
        );
    }

    #[test]
    fn test_rust_variables() {
        let got: BTreeMap<String, String> =
            wanted_variables(&[Language::Rust], &lay(), Platform::Other)
                .into_iter()
                .collect();
        assert_eq!(
            got["CARGO_BUILD_BUILD_DIR"],
            format!("{}/shared/cargo-build", lay().root)
        );
        assert_eq!(got["CARGO_TARGET_DIR"], format!("{}/target", lay().wt));
        assert!(got.contains_key("WORKTREE_BUILDS_ROOT"));
    }

    #[test]
    fn test_uv_link_mode_only_on_darwin() {
        let names = |platform: Platform| -> Vec<String> {
            wanted_variables(&[Language::Python], &lay(), platform)
                .into_iter()
                .map(|(n, _)| n)
                .collect()
        };
        assert!(names(Platform::Darwin).contains(&"UV_LINK_MODE".to_owned()));
        assert!(!names(Platform::Other).contains(&"UV_LINK_MODE".to_owned()));
    }
}

mod env_plan {
    use super::*;

    fn wanted_at(layout: &Layout) -> Vec<(String, String)> {
        wanted_variables(&[Language::Rust], layout, Platform::Other)
    }

    fn wanted() -> Vec<(String, String)> {
        wanted_at(&lay())
    }

    #[test]
    fn test_fresh_environment_exports_everything() {
        let plan = env_plan(&wanted(), &[Language::Rust], &Env::new());
        let names = exports(&plan);
        assert_eq!(names["CARGO_TARGET_DIR"], format!("{}/target", lay().wt));
        assert_eq!(
            names[names::BUILDS_SET_VAR],
            "CARGO_BUILD_BUILD_DIR:CARGO_TARGET_DIR:WORKTREE_BUILDS_ROOT:WORKTREE_BUILDS_WT"
        );
    }

    #[test]
    fn test_a_persons_variable_is_kept() {
        let plan = env_plan(
            &wanted(),
            &[Language::Rust],
            &env(&[("CARGO_TARGET_DIR", "/mine")]),
        );
        assert!(!exports(&plan).contains_key("CARGO_TARGET_DIR"));
        assert!(!exports(&plan)[names::BUILDS_SET_VAR].contains("CARGO_TARGET_DIR"));
    }

    #[test]
    fn test_inherited_from_another_worktree_is_recomputed() {
        let other = lay_at("/w/repo.worktrees/b", false);
        let first = exports(&env_plan(
            &wanted_at(&other),
            &[Language::Rust],
            &Env::new(),
        ));
        let plan = env_plan(&wanted(), &[Language::Rust], &first);
        assert_eq!(
            exports(&plan)["CARGO_TARGET_DIR"],
            format!("{}/target", lay().wt)
        );
    }

    #[test]
    fn test_same_worktree_leaves_its_own_alone() {
        let first = exports(&env_plan(&wanted(), &[Language::Rust], &Env::new()));
        let changed: Env = first
            .into_iter()
            .chain([("CARGO_TARGET_DIR".to_owned(), "/changed".to_owned())])
            .collect();
        let plan = env_plan(&wanted(), &[Language::Rust], &changed);
        assert!(!exports(&plan).contains_key("CARGO_TARGET_DIR"));
        assert!(exports(&plan)[names::BUILDS_SET_VAR].contains("CARGO_TARGET_DIR"));
    }

    #[test]
    fn test_names_no_longer_wanted_are_unset() {
        let other = lay_at("/w/repo.worktrees/b", false);
        let first = exports(&env_plan(
            &wanted_variables(&[Language::Python], &other, Platform::Other),
            &[Language::Python],
            &Env::new(),
        ));
        let plan = env_plan(&wanted(), &[Language::Rust], &first);
        assert!(plan.unsets.contains(&"RUFF_CACHE_DIR".to_owned()));
    }

    #[test]
    fn test_gradle_flag_is_appended_once() {
        let wanted = wanted_variables(&[Language::Gradle], &lay(), Platform::Other);
        let plan = env_plan(
            &wanted,
            &[Language::Gradle],
            &env(&[("GRADLE_OPTS", "-Xmx2g")]),
        );
        assert_eq!(
            exports(&plan)["GRADLE_OPTS"],
            format!("-Xmx2g {GRADLE_CACHING}")
        );
        let again = env_plan(
            &wanted,
            &[Language::Gradle],
            &env(&[("GRADLE_OPTS", &format!("-Xmx2g {GRADLE_CACHING}"))]),
        );
        assert!(!exports(&again).contains_key("GRADLE_OPTS"));
    }

    #[test]
    fn test_render_quotes() {
        let plan = EnvPlan {
            exports: vec![("A".to_owned(), "it's".to_owned())],
            unsets: strings(&["B"]),
        };
        assert_eq!(
            render_env(&plan, Shell::Sh),
            "unset B\nexport A='it'\\''s'\n"
        );
        assert_eq!(
            render_env(&plan, Shell::Fish),
            "set -e B\nset -gx A 'it\\'s'\n"
        );
    }
}

mod provisioning {
    use super::*;

    #[test]
    fn test_copy_and_allow() {
        assert_eq!(
            provision_actions(
                false,
                false,
                true,
                Some(b"use flake\n"),
                Some(b"use flake\n"),
                true
            ),
            vec![ProvisionAction::CopySettings, ProvisionAction::DirenvAllow]
        );
    }

    #[test]
    fn test_differing_envrc_is_not_allowed() {
        assert_eq!(
            provision_actions(false, true, true, Some(b"a"), Some(b"b"), true),
            Vec::new()
        );
    }

    #[test]
    fn test_main_checkout_needs_nothing() {
        assert_eq!(
            provision_actions(true, false, true, Some(b"a"), Some(b"a"), true),
            Vec::new()
        );
    }
}

mod parsing {
    use super::*;

    #[test]
    fn test_worktree_list() {
        let text = concat!(
            "worktree /w/repo\nHEAD aaa\nbranch refs/heads/main\n\n",
            "worktree /w/x\nHEAD bbb\ndetached\nlocked claude agent a (pid 1 start x)\n\n",
            "worktree /w/y\nHEAD ccc\nbranch refs/heads/y\nprunable gitdir file points to non-existent location\n",
        );
        let [main, x, y] = &parse_worktree_list(text)[..] else {
            panic!("three worktrees");
        };
        assert!(main.is_main);
        assert_eq!(main.branch.as_deref(), Some("main"));
        assert!(x.detached);
        assert_eq!(x.locked.as_deref(), Some("claude agent a (pid 1 start x)"));
        assert!(y.prunable);
        assert!(!y.is_main);
    }

    #[test]
    fn test_locked_without_a_reason_is_still_locked() {
        let [_, x] =
            &parse_worktree_list("worktree /w/r\nHEAD a\n\nworktree /w/x\nHEAD b\nlocked\n")[..]
        else {
            panic!("two worktrees");
        };
        assert_eq!(x.locked.as_deref(), Some(""));
    }

    #[test]
    fn test_pattern_file() {
        assert_eq!(
            parse_pattern_file("# c\n\nsecret.db\n  \n*.key \n"),
            strings(&["secret.db", "*.key"])
        );
    }
}

mod clean {
    use super::*;

    #[test]
    fn test_refusals() {
        assert_eq!(
            clean_refusals::<&str>(true, false, &[]),
            strings(&["the main checkout"])
        );
        assert_eq!(
            clean_refusals(false, false, &[" M a.txt", "?? new.txt"]),
            strings(&["1 tracked files with changes"])
        );
        assert_eq!(
            clean_refusals(false, false, &["?? new.txt"]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_plan_spares_kept_files_and_nested_repositories() {
        let kept = BTreeSet::from([".claude/settings.local.json".to_owned()]);
        let (whole, mixed) = clean_plan(
            &["build/", "junk.o", ".claude/", "nested/"],
            &kept,
            &["nested/"],
        );
        assert_eq!(whole, strings(&["build/", "junk.o"]));
        assert_eq!(mixed, strings(&[".claude/"]));
        assert_eq!(
            files_to_remove(
                &[
                    ".claude/settings.local.json",
                    ".claude/cache.txt",
                    ".claude/x/"
                ],
                &kept,
            ),
            strings(&[".claude/cache.txt"])
        );
    }
}

mod formatting {
    use super::*;

    #[test]
    fn test_human() {
        assert_eq!(human_bytes(Some(2048)), "2K");
        assert_eq!(human_bytes(None), "?");
        assert_eq!(human_age(Some(90.0)), "1m");
        assert_eq!(human_age(Some(2.0 * 86400.0)), "2d");
    }

    #[test]
    fn test_merge_config_and_unknown_keys() {
        let merged = merge_config(
            &object(json!({"a": 1, "s": {"x": 1, "y": 2}})),
            &object(json!({"s": {"y": 3}})),
        );
        assert_eq!(merged, object(json!({"a": 1, "s": {"x": 1, "y": 3}})));
        assert_eq!(
            unknown_keys(
                &object(json!({"s": {"x": 1}})),
                &object(json!({"s": {"z": 1}, "q": 2}))
            ),
            strings(&["s.z", "q"])
        );
    }
}
