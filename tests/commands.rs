//! Tests of env, provision, clean, sessions and the entry point, over
//! temporary repositories.

mod sandbox;

use std::collections::BTreeMap;
use std::time::Instant;

use git_worktree_manager::{build_root, names};
use sandbox::{BIN, Sandbox, exists, join, read, real, utime};
use serde_json::Value;

/// Default keep patterns, each with a path it keeps.
const KEPT_SAMPLES: [(&str, &str); 18] = [
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
];

const X: &str = "x\n";

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

// Env -----------------------------------------------------------------------------

fn env_vars(
    s: &Sandbox,
    cwd: &str,
    extra: &[(&str, &str)],
    args: &[&str],
) -> BTreeMap<String, String> {
    let argv: Vec<&str> = ["env", "--json"]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    let result = s.wt(&argv, Some(cwd), extra);
    assert_eq!(result.code, 0, "{}", result.stderr);
    let data: Value = serde_json::from_str(&result.stdout).expect("JSON");
    serde_json::from_value(data["exports"].clone()).expect("exports")
}

#[test]
fn test_two_worktrees_share_a_root_and_differ_in_target() {
    let s = Sandbox::new();
    let repo = s.repo(&[("Cargo.toml", "[package]\n")]);
    let (a, b) = (s.add_worktree(&repo, "a"), s.add_worktree(&repo, "b"));
    let (va, vb) = (env_vars(&s, &a, &[], &[]), env_vars(&s, &b, &[], &[]));
    assert_eq!(va["WORKTREE_BUILDS_ROOT"], vb["WORKTREE_BUILDS_ROOT"]);
    assert_eq!(va["CARGO_BUILD_BUILD_DIR"], vb["CARGO_BUILD_BUILD_DIR"]);
    assert_ne!(va["CARGO_TARGET_DIR"], vb["CARGO_TARGET_DIR"]);
    assert!(va["CARGO_TARGET_DIR"].starts_with(&format!("{}/wt/a-", va["WORKTREE_BUILDS_ROOT"])));
    assert!(env_vars(&s, &repo, &[], &[])["WORKTREE_BUILDS_WT"].ends_with("/wt/main"));
}

#[test]
fn test_outside_git_prints_nothing() {
    let s = Sandbox::new();
    let outside = join(&s.tmp, "plain");
    std::fs::create_dir_all(&outside).expect("a folder");
    let result = s.wt(&["env"], Some(&outside), &[]);
    assert_eq!((result.code, result.stdout.as_str()), (0, ""));
}

#[test]
fn test_a_persons_variable_is_kept() {
    let s = Sandbox::new();
    let repo = s.repo(&[("Cargo.toml", "[package]\n")]);
    let got = env_vars(&s, &repo, &[("CARGO_TARGET_DIR", "/mine")], &[]);
    assert!(!got.contains_key("CARGO_TARGET_DIR"));
    assert!(got.contains_key("CARGO_BUILD_BUILD_DIR"));
}

#[test]
fn test_languages_option_and_unknown() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    assert!(env_vars(&s, &repo, &[], &["--languages", "tofu"]).contains_key("TF_DATA_DIR"));
    assert_eq!(
        s.wt(&["env", "--languages", "cobol"], Some(&repo), &[])
            .code,
        2
    );
}

#[test]
fn test_fish_syntax() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    assert!(
        s.wt(&["env", "--shell", "fish"], Some(&repo), &[])
            .stdout
            .contains("set -gx WORKTREE_BUILDS_ROOT ")
    );
}

#[test]
fn test_first_entry_provisions() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(
        &join(&repo, ".claude/settings.local.json"),
        "{\"permissions\": {}}\n",
    );
    let wt = s.add_worktree(&repo, "feat");
    let gitdir = s
        .git(&wt, &["rev-parse", "--absolute-git-dir"])
        .trim()
        .to_owned();
    assert!(!exists(&join(&gitdir, names::PROVISIONED_MARKER)));
    let vars = env_vars(&s, &wt, &[], &[]);
    assert!(exists(&join(&gitdir, names::PROVISIONED_MARKER)));
    assert_eq!(
        read(&join(&wt, ".claude/settings.local.json")),
        "{\"permissions\": {}}\n"
    );
    assert_eq!(
        read(&join(&vars["WORKTREE_BUILDS_WT"], "WORKTREE")).trim(),
        wt
    );
}

#[test]
fn test_provision_never_overwrites_settings_and_dry_run_writes_nothing() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, ".claude/settings.local.json"), "main\n");
    let wt = s.add_worktree(&repo, "feat");
    assert_eq!(
        s.wt_json(&["provision", "--dry-run", &wt], None, 0)["would"],
        true
    );
    assert!(!exists(&join(&wt, ".claude")));
    s.write(&join(&wt, ".claude/settings.local.json"), "mine\n");
    s.wt_json(&["provision", &wt], None, 0);
    assert_eq!(read(&join(&wt, ".claude/settings.local.json")), "mine\n");
}

/// The one-minute load average, from the kernel; None where neither Linux's
/// nor macOS's source answers.
fn load_average() -> Option<f64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok().or_else(|| {
        std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
    })?;
    text.split_whitespace().find_map(|word| word.parse().ok())
}

#[test]
fn test_env_is_fast_once_provisioned() {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    if let Some(load) = load_average().filter(|load| *load > cores as f64) {
        eprintln!("skipped: load average {load:.0} is above {cores} cores");
        return;
    }
    let s = Sandbox::new();
    let repo = s.repo(&[("Cargo.toml", "[package]\n")]);
    let wt = s.add_worktree(&repo, "feat");
    s.wt(&["env"], Some(&wt), &[]);
    let once = |program: &str, args: &[&str]| {
        let start = Instant::now();
        let status = std::process::Command::new(program)
            .args(args)
            .current_dir(&wt)
            .env_clear()
            .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("runs");
        assert!(status.success());
        start.elapsed().as_secs_f64()
    };
    // A process that does nothing is the floor, as a bare interpreter was
    // for the Python program.
    let pairs: Vec<(f64, f64)> = (0..15)
        .map(|_| (once("true", &[]), once(BIN, &["env"])))
        .collect();
    let bare = pairs.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let env = pairs.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    assert!(
        env - bare < 0.050,
        "env took {:.0} ms above a process that does nothing",
        (env - bare) * 1000.0
    );
}

// Clean ---------------------------------------------------------------------------

fn populated(s: &Sandbox) -> String {
    let repo = s.repo(&[(".worktree-keep", "secret.db\n")]);
    s.write(&join(&repo, ".claude/settings.local.json"), "main\n");
    let wt = s.add_worktree(&repo, "feat");
    s.commit(&wt, "work.txt", X);
    KEPT_SAMPLES
        .iter()
        .for_each(|(_, rel)| s.write(&join(&wt, rel), &format!("kept {rel}\n")));
    [
        "secret.db",
        "build/out.o",
        "junk.o",
        ".claude/cache.txt",
        "state/other",
        "skills/notes.txt",
    ]
    .iter()
    .for_each(|rel| s.write(&join(&wt, rel), X));
    let nested = join(&wt, "nested");
    s.sh(&["git", "init", "--quiet", &nested], None);
    s.write(&join(&nested, "inner.o"), "inner\n");
    let buried = join(&wt, "build/dep");
    s.sh(&["git", "init", "--quiet", &buried], None);
    s.write(&join(&buried, "src.c"), X);
    wt
}

#[test]
fn test_keeps_defaults_and_worktree_keep_and_deletes_the_rest() {
    let s = Sandbox::new();
    let wt = populated(&s);
    let exported: Value =
        serde_json::from_str(&s.wt(&["env", "--json"], Some(&wt), &[]).stdout).expect("JSON");
    let build = exported["exports"]["WORKTREE_BUILDS_WT"]
        .as_str()
        .expect("the build folder")
        .to_owned();
    s.write(&join(&build, "target/debug/app"), X);
    let listing = s.wt_json(&["clean", "--dry-run", &wt], None, 0);
    assert_eq!(listing["would"], true);
    assert!(exists(&join(&wt, "junk.o")));
    let done = s.wt_json(&["clean", &wt], None, 0);
    KEPT_SAMPLES.iter().for_each(|(pattern, rel)| {
        assert!(exists(&join(&wt, rel)), "{rel} ({pattern}) was deleted");
    });
    ["secret.db", "work.txt", ".worktree-keep"]
        .iter()
        .for_each(|rel| assert!(exists(&join(&wt, rel)), "{rel}"));
    [
        "build/out.o",
        "junk.o",
        ".claude/cache.txt",
        "state/other",
        "skills/notes.txt",
    ]
    .iter()
    .for_each(|rel| assert!(!exists(&join(&wt, rel)), "{rel}"));
    assert!(exists(&join(&wt, "nested/inner.o")));
    assert!(exists(&join(&wt, "build/dep/src.c")));
    assert!(!exists(&build));
    assert!(strings(&done["remove"]).contains(&"junk.o"));
    let cleaned: Vec<String> = s
        .events(&wt)
        .iter()
        .filter(|e| e["event"] == "clean")
        .map(|e| e["worktree"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(cleaned, vec![wt]);
}

#[test]
fn test_main_checkout_is_refused() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.write(&join(&repo, "junk.o"), X);
    let refused = s.wt_json(&["clean", &repo], None, 3);
    assert!(strings(&refused["reasons"]).contains(&"the main checkout"));
    assert!(exists(&join(&repo, "junk.o")));
}

#[test]
fn test_dirty_tree_is_refused() {
    let s = Sandbox::new();
    let repo = s.repo(&[("a.txt", "a\n")]);
    let wt = s.add_worktree(&repo, "feat");
    s.write(&join(&wt, "a.txt"), "changed\n");
    s.write(&join(&wt, "junk.o"), X);
    let refused = s.wt_json(&["clean", &wt], None, 3);
    assert!(strings(&refused["reasons"]).contains(&"1 tracked files with changes"));
    assert!(exists(&join(&wt, "junk.o")));
}

// SweepCleansTheIdle ----------------------------------------------------------------

#[test]
#[ignore = "sweep is ported by rust-sweep-through-git"]
fn test_an_unlanded_worktree_idle_a_day_loses_its_build_output() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let wt = s.add_worktree(&repo, "wip");
    s.commit(&wt, "wip.txt", X);
    s.write(&join(&wt, "target/debug/app"), X);
    s.write(&join(&wt, ".env"), "SECRET=1\n");
    let gitdir = s
        .git(&wt, &["rev-parse", "--absolute-git-dir"])
        .trim()
        .to_owned();
    let old = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock")
        .as_secs_f64()
        - 2.0 * 86400.0;
    ["HEAD", "index", "logs/HEAD"]
        .iter()
        .for_each(|name| utime(&join(&gitdir, name), old));
    let report = s.wt_json(&["sweep", &repo], None, 0);
    let entry = s.entry(&report, "worktrees", &wt);
    assert_eq!(entry["action"], "clean");
    assert!(!exists(&join(&wt, "target")));
    assert!(exists(&join(&wt, ".env")));
    assert!(s.listed(&repo).contains(&wt));
}

// Sessions --------------------------------------------------------------------------

#[test]
fn test_sessions_newest_first() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let wt = s.add_worktree(&repo, "feat");
    let projects = join(&s.var("CLAUDE_CONFIG_DIR"), "projects");
    let old = join(
        &join(&projects, &build_root::claude_project_folder(&real(&repo))),
        "old.jsonl",
    );
    let new = join(
        &join(&projects, &build_root::claude_project_folder(&wt)),
        "new.jsonl",
    );
    s.write(&old, X);
    s.write(&new, X);
    utime(&old, 1000.0);
    s.write(&join(&projects, "-unrelated/x.jsonl"), X);
    let data = s.wt_json(&["sessions"], Some(&wt), 0);
    let names: Vec<&str> = data["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|session| session["session"].as_str())
        .collect();
    assert_eq!(names, ["new", "old"]);
}

// EntryPoint ------------------------------------------------------------------------

#[test]
fn test_every_command_has_help() {
    let s = Sandbox::new();
    ["env", "provision", "clean", "sessions"]
        .iter()
        .for_each(|command| {
            let result = s.wt(&[command, "--help"], None, &[]);
            assert_eq!(result.code, 0, "{command}");
            assert!(result.stdout.contains("Example:"), "{command}");
        });
}

#[test]
fn test_abbreviated_options_are_refused() {
    let s = Sandbox::new();
    assert_eq!(s.wt(&["provision", "--dry"], None, &[]).code, 2);
}

#[test]
fn test_the_sh_entry_point_runs_the_package_through_a_symbolic_link() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let link = join(&s.tmp, &format!("bin/{}", git_worktree_manager::PROGRAM));
    std::fs::create_dir_all(join(&s.tmp, "bin")).expect("a folder");
    std::os::unix::fs::symlink(BIN, &link).expect("a link");
    let result = std::process::Command::new(&link)
        .args(["sessions", "--json"])
        .current_dir(&repo)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .output()
        .expect("runs");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let data: Value = serde_json::from_slice(&result.stdout).expect("JSON");
    assert_eq!(data["checkout"], real(&repo));
}
