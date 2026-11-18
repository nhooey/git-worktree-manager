//! `env`, `provision` and `clean`, run as the binary in sandbox
//! repositories.

mod sandbox;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use git_worktree_manager::build_root;
use git_worktree_manager::names::PROGRAM;
use sandbox::{Sandbox, exists, join, read};
use serde_json::Value;

/// What a tree holds, without following links: each path's kind, size and
/// modification time.
fn snapshot(root: &str) -> BTreeMap<PathBuf, (String, u64, Option<SystemTime>)> {
    fn walk(dir: &Path, into: &mut BTreeMap<PathBuf, (String, u64, Option<SystemTime>)>) {
        for entry in fs::read_dir(dir).expect("a folder") {
            let path = entry.expect("an entry").path();
            let meta = fs::symlink_metadata(&path).expect("metadata");
            let kind = format!("{:?}", meta.file_type());
            into.insert(path.clone(), (kind, meta.len(), meta.modified().ok()));
            if meta.is_dir() {
                walk(&path, into);
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(Path::new(root), &mut found);
    found
}

/// The value `env --format json` gives `name` in `worktree`.
fn exported(s: &Sandbox, worktree: &str, name: &str) -> String {
    let report = s.run_json(&["env", "--languages", "rust,c"], Some(worktree), 0);
    report["exports"]
        .as_array()
        .expect("exports")
        .iter()
        .find(|export| export["name"] == name)
        .and_then(|export| export["value"].as_str())
        .unwrap_or_else(|| panic!("{name} in {report}"))
        .to_owned()
}

fn link_exists(path: &str) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Whether `program` is on the test's own `PATH`.
fn on_path(program: &str) -> bool {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .any(|folder| Path::new(folder).join(program).is_file())
}

/// A clone of a bare remote, and a linked worktree of the bare one.
fn bare_with_worktree(s: &Sandbox) -> (String, String) {
    let _clone = s.cloned();
    let remote = join(&s.tmp, "repo.remote.git");
    let target = join(&s.tmp, "bare-linked");
    s.git(
        &remote,
        &[
            "worktree", "add", "--quiet", "-b", "linked", &target, "main",
        ],
    );
    (sandbox::real(&remote), sandbox::real(&target))
}

// env -------------------------------------------------------------------------

#[test]
fn test_env_prints_the_exports_and_writes_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[("Cargo.toml", "[package]\n")]);
    let wt = s.add_worktree(&repo, "feat");
    let before = snapshot(&s.tmp);
    let ran = s.run(&["env"], Some(&wt), &[]);
    let json = s.run_json(&["env", "--shell", "fish"], Some(&wt), 0);
    let fish = s.run(&["env", "--shell", "fish"], Some(&wt), &[]);
    assert_eq!(snapshot(&s.tmp), before, "env changed the sandbox");
    assert!(!exists(&s.var("XDG_CACHE_HOME")), "env made the build root");

    assert_eq!((ran.code, ran.stderr.as_str()), (0, ""));
    let dir = exported(&s, &wt, &build_root::dir_variable());
    assert!(
        dir.starts_with(&join(&s.var("XDG_CACHE_HOME"), PROGRAM)),
        "{dir}"
    );
    assert!(dir.contains("/wt/feat-"), "{dir}");
    assert!(
        ran.stdout
            .contains(&format!("export CARGO_TARGET_DIR='{dir}/target'\n")),
        "{}",
        ran.stdout
    );
    assert_eq!(json["languages"], serde_json::json!(["rust"]));
    assert_eq!(fish.code, 0);
    assert!(
        fish.stdout.contains(&format!(
            "set --global --export CARGO_TARGET_DIR '{dir}/target'\n"
        )),
        "{}",
        fish.stdout
    );
}

#[test]
fn test_env_outside_a_repository_prints_nothing_and_exits_1() {
    let s = Sandbox::new();
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    for args in [&["env"][..], &["env", "--format", "json"]] {
        let ran = s.run(args, Some(&plain), &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{args:?}");
        assert!(
            ran.stderr.contains("is not in a repository"),
            "{}",
            ran.stderr
        );
    }
}

#[test]
fn test_env_refuses_an_unknown_language_and_accepts_one_that_sets_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let ran = s.run(&["env", "--languages", "rust cobol"], Some(&repo), &[]);
    assert_eq!((ran.code, ran.stdout.as_str()), (2, ""));
    assert!(
        ran.stderr.contains("unknown languages: cobol; known: c, "),
        "{}",
        ran.stderr
    );
    let go = s.run_json(&["env", "--languages", "Go,TS"], Some(&repo), 0);
    assert_eq!(go["languages"], serde_json::json!(["go", "javascript"]));
}

/// A worktree whose path holds a space, `'`, `$`, a backtick and a
/// newline, and the lines `env --shell <shell>` prints in it.
fn awkward(s: &Sandbox, shell: &str) -> (String, String) {
    let repo = s.repo(&[]);
    let target = join(&s.tmp, "a b'c$d`e\nf");
    s.git(
        &repo,
        &["worktree", "add", "--quiet", "-b", "odd", &target, "HEAD"],
    );
    let wt = sandbox::real(&target);
    let ran = s.run(
        &["env", "--languages", "c,rust", "--shell", shell],
        Some(&wt),
        &[],
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    (wt, ran.stdout)
}

#[test]
fn test_sh_reads_the_lines_back_whatever_the_path_holds() {
    let s = Sandbox::new();
    let (wt, lines) = awkward(&s, "sh");
    let read_back = s.sh(
        &[
            "sh",
            "-c",
            "eval \"$1\"; printf '%s|%s' \"$CCACHE_BASEDIR\" \"$CARGO_TARGET_DIR\"",
            "sh",
            &lines,
        ],
        None,
    );
    let target = exported(&s, &wt, "CARGO_TARGET_DIR");
    assert_eq!(read_back, format!("{wt}|{target}"));
}

#[test]
fn test_fish_reads_the_lines_back_whatever_the_path_holds() {
    if !on_path("fish") {
        // The flake's checks give the tests fish, so there it never skips.
        assert!(
            std::env::var_os("NIX_BUILD_TOP").is_none(),
            "fish is not on PATH in a Nix build"
        );
        eprintln!("fish is not on PATH: skipped");
        return;
    }
    let s = Sandbox::new();
    let (wt, lines) = awkward(&s, "fish");
    let read_back = s.sh(
        &[
            "fish",
            "--no-config",
            "--command",
            "printf '%s\\n' $argv[1] | source; printf '%s|%s' \"$CCACHE_BASEDIR\" \"$CARGO_TARGET_DIR\"",
            &lines,
        ],
        None,
    );
    let target = exported(&s, &wt, "CARGO_TARGET_DIR");
    assert_eq!(read_back, format!("{wt}|{target}"));
}

// provision ---------------------------------------------------------------------

const SETTINGS: &str = ".claude/settings.local.json";

/// A folder for `PATH` holding nothing but the stubs `programs`, each
/// logging its arguments to `<tmp>/<name>.log` and exiting `code`; the
/// variables to run the program with.
fn stubs(s: &Sandbox, programs: &[(&str, i32)]) -> Vec<(String, String)> {
    let bin = join(&s.tmp, "bin");
    fs::create_dir_all(&bin).expect("a folder");
    for (name, code) in programs {
        let program = join(&bin, name);
        let log = join(&s.tmp, &format!("{name}.log"));
        s.write(
            &program,
            &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{log}'\nexit {code}\n"),
        );
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).expect("executable");
    }
    vec![("PATH".to_owned(), bin)]
}

fn log(s: &Sandbox, program: &str) -> String {
    join(&s.tmp, &format!("{program}.log"))
}

fn provision(s: &Sandbox, wt: &str, env: &[(String, String)]) -> (i32, Value, String) {
    let pairs: Vec<(&str, &str)> = env.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let ran = s.run(&["provision", wt, "--format", "json"], None, &pairs);
    let report = serde_json::from_str(&ran.stdout).unwrap_or(Value::Null);
    (ran.code, report, ran.stderr)
}

/// The outcome and detail of `step` in a provision report.
fn step(report: &Value, name: &str) -> (String, String) {
    report["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .find(|step| step["step"] == name)
        .map(|step| {
            (
                step["outcome"].as_str().unwrap_or_default().to_owned(),
                step["detail"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .unwrap_or_else(|| panic!("{name} in {report}"))
}

fn outcome(report: &Value, name: &str) -> String {
    step(report, name).0
}

#[test]
fn test_provision_copies_settings_and_never_overwrites_them() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, SETTINGS), "{\"main\": true}\n");
    let wt = s.add_worktree(&repo, "feat");
    let env = stubs(&s, &[("direnv", 0)]);

    let (code, report, stderr) = provision(&s, &wt, &env);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(outcome(&report, "settings"), "done");
    assert_eq!(outcome(&report, "direnv"), "skipped");
    assert_eq!(read(&join(&wt, SETTINGS)), "{\"main\": true}\n");

    s.write(&join(&wt, SETTINGS), "{\"mine\": true}\n");
    let (code, report, _) = provision(&s, &wt, &env);
    assert_eq!(code, 0);
    assert_eq!(outcome(&report, "settings"), "skipped");
    assert_eq!(read(&join(&wt, SETTINGS)), "{\"mine\": true}\n");
}

#[test]
fn test_provision_allows_only_the_main_checkouts_envrc() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".envrc"), "use flake\n");
    let same = s.add_worktree(&repo, "same");
    let other = s.add_worktree(&repo, "other");
    s.write(&join(&same, ".envrc"), "use flake\n");
    s.write(&join(&other, ".envrc"), "use flake\ncurl evil | sh\n");
    let env = stubs(&s, &[("direnv", 0)]);

    let (code, report, stderr) = provision(&s, &other, &env);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(outcome(&report, "direnv"), "skipped");
    assert!(
        !exists(&log(&s, "direnv")),
        "direnv ran for a changed .envrc"
    );

    let (code, report, stderr) = provision(&s, &same, &env);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(outcome(&report, "direnv"), "done");
    assert_eq!(read(&log(&s, "direnv")), format!("allow {same}\n"));
}

#[test]
fn test_provision_reports_a_failed_step_runs_the_rest_and_exits_1() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".envrc"), "use flake\n");
    s.write(&join(&repo, SETTINGS), "{}\n");
    let wt = s.add_worktree(&repo, "feat");
    s.write(&join(&wt, ".envrc"), "use flake\n");
    let env = stubs(&s, &[("direnv", 1)]);
    let (code, report, stderr) = provision(&s, &wt, &env);
    assert_eq!(code, 1);
    assert_eq!(outcome(&report, "direnv"), "failed");
    assert_eq!(outcome(&report, "settings"), "done");
    assert!(stderr.contains("direnv allow exited 1"), "{stderr}");
}

#[test]
fn test_provision_skips_direnv_when_it_is_not_on_path() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".envrc"), "use flake\n");
    let wt = s.add_worktree(&repo, "feat");
    s.write(&join(&wt, ".envrc"), "use flake\n");
    let (code, report, stderr) = provision(&s, &wt, &stubs(&s, &[]));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        step(&report, "direnv"),
        ("skipped".to_owned(), "direnv is not on PATH".to_owned())
    );
}

#[test]
fn test_provision_sets_up_the_build_root_once() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let wt = s.add_worktree(&repo, "feat");
    let env = stubs(&s, &[("tmutil", 0)]);
    let root = exported(&s, &wt, &build_root::root_variable());
    let dir = exported(&s, &wt, &build_root::dir_variable());
    let top = Path::new(&root)
        .parent()
        .expect("a folder")
        .to_string_lossy()
        .into_owned();

    let (code, first, stderr) = provision(&s, &wt, &env);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        read(&join(&root, "REPO")),
        format!("{}\n", join(&repo, ".git"))
    );
    assert_eq!(build_root::WORKTREE_FILE, "WORKTREE", "the sweep reads it");
    assert_eq!(
        Path::new(&dir).parent(),
        Some(Path::new(&join(&root, "wt"))),
        "the back-pointer's folder is <build root>/wt/<id>"
    );
    assert_eq!(
        read(&join(&dir, "WORKTREE")),
        format!("{wt}\n"),
        "the back-pointer is the worktree's real path and one newline"
    );
    let (_, second, _) = provision(&s, &wt, &env);
    for name in ["repo", "worktree", "never-index", "time-machine"] {
        assert_eq!(outcome(&second, name), "skipped", "{name}");
    }
    if cfg!(target_os = "macos") {
        assert_eq!(outcome(&first, "never-index"), "done");
        assert_eq!(outcome(&first, "time-machine"), "done");
        assert!(exists(&join(&top, ".metadata_never_index")));
        assert_eq!(read(&log(&s, "tmutil")), format!("addexclusion {top}\n"));
    } else {
        assert_eq!(outcome(&first, "never-index"), "skipped");
        assert!(!exists(&log(&s, "tmutil")));
    }
}

#[test]
fn test_time_machine_is_tried_again_until_the_exclusion_is_made() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let wt = s.add_worktree(&repo, "feat");
    let (code, first, _) = provision(&s, &wt, &stubs(&s, &[("tmutil", 1)]));
    assert_eq!(code, 1);
    assert_eq!(outcome(&first, "never-index"), "done");
    assert_eq!(outcome(&first, "time-machine"), "failed");
    let (code, second, stderr) = provision(&s, &wt, &stubs(&s, &[("tmutil", 0)]));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(outcome(&second, "never-index"), "skipped");
    assert_eq!(outcome(&second, "time-machine"), "done");
    let (_, third, _) = provision(&s, &wt, &stubs(&s, &[("tmutil", 0)]));
    assert_eq!(
        step(&third, "time-machine"),
        ("skipped".to_owned(), "it is excluded already".to_owned())
    );
}

#[test]
fn test_a_path_entry_that_is_no_folder_holds_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".envrc"), "use flake\n");
    let wt = s.add_worktree(&repo, "feat");
    s.write(&join(&wt, ".envrc"), "use flake\n");
    let file = join(&s.tmp, "a-file");
    s.write(&file, "not a folder\n");
    let path = stubs(&s, &[("direnv", 0)])
        .into_iter()
        .map(|(name, bin)| (name, format!("{file}:{bin}")))
        .collect::<Vec<_>>();
    let (code, report, stderr) = provision(&s, &wt, &path);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(outcome(&report, "direnv"), "done");
}

#[test]
fn test_a_back_pointer_with_other_content_is_replaced_and_said_so() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let wt = s.add_worktree(&repo, "feat");
    let dir = exported(&s, &wt, &build_root::dir_variable());
    s.write(&join(&dir, "WORKTREE"), "/elsewhere\n");
    let (code, report, stderr) = provision(&s, &wt, &stubs(&s, &[]));
    assert_eq!(code, 0, "{stderr}");
    let (done, detail) = step(&report, "worktree");
    assert_eq!(done, "done");
    assert!(detail.ends_with(", replacing \"/elsewhere\""), "{detail}");
    assert_eq!(read(&join(&dir, "WORKTREE")), format!("{wt}\n"));
}

#[test]
fn test_a_back_pointer_is_the_paths_raw_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/src/caf\xe9"));
    assert_eq!(
        git_worktree_manager::commands::provision::line(path),
        b"/src/caf\xe9\n"
    );
}

#[test]
fn test_provision_leaves_the_main_checkout_alone() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".envrc"), "use flake\n");
    let env = stubs(&s, &[("direnv", 0)]);
    let (code, report, _) = provision(&s, &repo, &env);
    assert_eq!(code, 0);
    assert_eq!(outcome(&report, "settings"), "skipped");
    assert_eq!(outcome(&report, "direnv"), "skipped");
    assert_eq!(outcome(&report, "worktree"), "done");
    assert!(!exists(&log(&s, "direnv")));
}

// clean ---------------------------------------------------------------------------

/// A linked worktree holding build output beside every kind of file
/// `clean` keeps, its folder in the build root, and the main checkout's
/// build output.
struct Built {
    s: Sandbox,
    repo: String,
    wt: String,
    build_dir: String,
    main_build_dir: String,
    outside: String,
}

const BUILD_OUTPUT: [&str; 4] = [
    "target/debug/app",
    "target/debug/deps/lib.o",
    "build/out.bin",
    ".claude/other.json",
];

const KEPT: [&str; 4] = [
    ".claude/settings.local.json",
    ".envrc",
    "config.local.toml",
    "target/notes.local.md",
];

fn built() -> Built {
    let s = Sandbox::new();
    let repo = s.repo(&[("src/main.rs", "fn main() {}\n")]);
    s.write(&join(&repo, "target/tracked.txt"), "forced\n");
    s.git(&repo, &["add", "--force", "--", "target/tracked.txt"]);
    s.git(
        &repo,
        &["commit", "--quiet", "-m", "add target/tracked.txt"],
    );
    s.write(&join(&repo, "target/app"), "main's\n");
    let wt = s.add_worktree(&repo, "feat");
    BUILD_OUTPUT
        .iter()
        .chain(&KEPT)
        .for_each(|rel| s.write(&join(&wt, rel), "x\n"));
    s.write(&join(&wt, "src/main.rs"), "fn main() { changed() }\n");
    s.write(&join(&wt, "notes.txt"), "untracked\n");
    s.sh(&["git", "init", "--quiet", &join(&wt, "nested/lib")], None);
    s.write(&join(&wt, "nested/lib/build.o"), "nested\n");
    let outside = join(&s.tmp, "outside");
    s.write(&join(&outside, "precious"), "keep me\n");
    symlink(&outside, join(&wt, "build/link")).expect("a link");
    let build_dir = exported(&s, &wt, &build_root::dir_variable());
    s.write(&join(&build_dir, "target/debug/app"), "x\n");
    let main_build_dir = exported(&s, &repo, &build_root::dir_variable());
    s.write(&join(&main_build_dir, "target/app"), "main's\n");
    Built {
        s,
        repo,
        wt,
        build_dir,
        main_build_dir,
        outside,
    }
}

impl Built {
    /// Whether the main checkout's build output is as `built` left it.
    fn main_untouched(&self) -> bool {
        read(&join(&self.repo, "target/app")) == "main's\n"
            && read(&join(&self.main_build_dir, "target/app")) == "main's\n"
    }
}

#[test]
fn test_clean_deletes_build_output_and_nothing_else() {
    let b = built();
    let report = b.s.run_json(&["clean", &b.wt], None, 0);
    assert_eq!(report["outcome"], "done", "{report}");

    for rel in BUILD_OUTPUT.iter().chain(&["build/link", "target/debug"]) {
        assert!(!link_exists(&join(&b.wt, rel)), "{rel} is left");
    }
    assert!(!exists(&join(&b.wt, "build")), "an emptied folder is left");
    assert!(!exists(&b.build_dir), "the build root's folder is left");
    for rel in KEPT {
        assert_eq!(read(&join(&b.wt, rel)), "x\n", "{rel}");
    }
    assert_eq!(read(&join(&b.wt, "target/tracked.txt")), "forced\n");
    assert_eq!(read(&join(&b.wt, "notes.txt")), "untracked\n");
    assert_eq!(
        read(&join(&b.wt, "src/main.rs")),
        "fn main() { changed() }\n"
    );
    assert_eq!(read(&join(&b.wt, "nested/lib/build.o")), "nested\n");
    assert_eq!(read(&join(&b.outside, "precious")), "keep me\n");
    assert_eq!(read(&join(&b.repo, "src/main.rs")), "fn main() {}\n");
    assert!(b.main_untouched());

    let again = b.s.run(&["clean", &b.wt], None, &[]);
    assert_eq!(again.code, 0, "{}", again.stderr);
    assert_eq!(again.stdout, format!("nothing to delete in {}\n", b.wt));
}

#[test]
fn test_clean_under_dry_run_changes_nothing_and_lists_what_it_would_delete() {
    let b = built();
    let before = snapshot(&b.s.tmp);
    let ran = b.s.run(&["clean", "--dry-run", &b.wt], None, &[]);
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert_eq!(snapshot(&b.s.tmp), before, "--dry-run changed the sandbox");
    for path in [
        join(&b.wt, "target/debug/app"),
        join(&b.wt, "target/debug/deps"),
        join(&b.wt, "target/debug"),
        join(&b.wt, "build"),
        b.build_dir.clone(),
    ] {
        assert!(
            ran.stdout.contains(&format!("would delete {path}\n")),
            "{path} in {}",
            ran.stdout
        );
    }
    assert!(
        !ran.stdout
            .contains(&format!("would delete {}\n", join(&b.wt, "target"))),
        "{}",
        ran.stdout
    );
}

#[test]
fn test_clean_keeps_the_main_checkout_and_a_locked_worktree_and_exits_3() {
    let b = built();
    let main = b.s.run_json(&["clean", &b.repo], None, 3);
    assert_eq!(main["outcome"], "none");
    assert_eq!(main["reasons"][0]["code"], "main-worktree");
    assert!(b.main_untouched());

    b.s.git(&b.repo, &["worktree", "lock", "--reason", "busy", &b.wt]);
    let ran = b.s.run(&["clean", &b.wt], None, &[]);
    assert_eq!(ran.code, 3, "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        format!("kept {}: locked: it is locked: busy\n", b.wt)
    );
    b.s.git(&b.repo, &["worktree", "unlock", &b.wt]);
    b.s.git(&b.repo, &["worktree", "lock", &b.wt]);
    let ran = b.s.run(&["clean", &b.wt], None, &[]);
    assert_eq!(ran.code, 3, "{}", ran.stderr);
    assert_eq!(
        ran.stdout,
        format!(
            "kept {}: locked: it is locked, with no reason given\n",
            b.wt
        )
    );
    assert!(exists(&join(&b.wt, "target/debug/app")));
    assert!(exists(&b.build_dir));
}

#[test]
fn test_a_lock_that_cannot_be_read_still_holds() {
    let b = built();
    b.s.git(&b.repo, &["worktree", "lock", "--reason", "busy", &b.wt]);
    let lock = join(&b.repo, ".git/worktrees/feat/locked");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o000)).expect("unreadable");
    let ran = b.s.run(&["clean", &b.wt], None, &[]);
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).expect("readable");
    assert_eq!(ran.code, 3, "{}{}", ran.stdout, ran.stderr);
    assert!(exists(&join(&b.wt, "target/debug/app")));
}

#[test]
fn test_git_dir_cannot_make_clean_treat_the_main_checkout_as_linked() {
    let b = built();
    let feat = join(&b.repo, ".git/worktrees/feat");
    let ran = b.s.run(
        &["clean", &b.repo],
        None,
        &[("GIT_DIR", &feat), ("GIT_WORK_TREE", &b.repo)],
    );
    assert_eq!(ran.code, 3, "{}{}", ran.stdout, ran.stderr);
    assert!(b.main_untouched());
}

#[test]
fn test_clean_takes_only_a_worktrees_root() {
    let b = built();
    let ran = b.s.run(&["clean", &join(&b.wt, "src")], None, &[]);
    assert_eq!((ran.code, ran.stdout.as_str()), (2, ""), "{}", ran.stderr);
    assert!(exists(&join(&b.wt, "target/debug/app")));
}

#[test]
fn test_clean_outside_a_repository_exits_1() {
    let s = Sandbox::new();
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    let ran = s.run(&["clean", &plain], None, &[]);
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert_eq!(
        ran.stderr,
        format!("{PROGRAM}: {plain} is not in a repository\n")
    );
}

// A bare repository ---------------------------------------------------------------

#[test]
fn test_a_linked_worktree_of_a_bare_repository() {
    let s = Sandbox::new();
    let (_, wt) = bare_with_worktree(&s);
    s.write(&join(&wt, "target/app"), "x\n");

    let dir = exported(&s, &wt, &build_root::dir_variable());
    assert!(dir.contains("/wt/bare-linked-"), "{dir}");

    let (code, report, stderr) = provision(&s, &wt, &stubs(&s, &[]));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        step(&report, "settings"),
        (
            "skipped".to_owned(),
            "the repository has no main checkout".to_owned()
        )
    );

    let cleaned = s.run_json(&["clean", &wt], None, 0);
    assert_eq!(cleaned["outcome"], "done", "{cleaned}");
    assert!(!exists(&join(&wt, "target")));
}

#[test]
fn test_a_bare_repository_has_no_worktree_for_any_command() {
    let s = Sandbox::new();
    let (bare, _) = bare_with_worktree(&s);
    for args in [
        vec!["env"],
        vec!["provision", bare.as_str()],
        vec!["clean", bare.as_str()],
        vec!["env", "--format", "json"],
    ] {
        let ran = s.run(&args, Some(&bare), &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{args:?}");
        assert!(
            ran.stderr.contains("is not in a worktree"),
            "{}",
            ran.stderr
        );
    }
}
