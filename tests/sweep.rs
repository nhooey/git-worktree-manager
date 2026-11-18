//! `sweep`, run as the binary over sandbox repositories: the archive
//! before a prune, orphaned build output, a failed archive, a dry run that
//! writes nothing, `--unattended`, and what moved between the plan and
//! the act.

mod sandbox;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use git_worktree_manager::build_root::{Layout, WORKTREE_FILE};
use git_worktree_manager::names::{self, LAST_SWEEP_FILE, SWEEP_LOCK_FILE, SWEEP_PENDING_FILE};
use sandbox::{Sandbox, exists, join, real, utime};
use serde_json::Value;

const FORMATS: [&str; 3] = ["text", "json", "markdown"];

// Helpers ------------------------------------------------------------------------------

/// The repository's common git directory, as a real path.
fn common_dir(s: &Sandbox, repo: &str) -> PathBuf {
    let dir = s.git(
        repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    PathBuf::from(real(dir.trim()))
}

fn state(s: &Sandbox, repo: &str) -> PathBuf {
    names::state_dir(&s.environment(), &common_dir(s, repo)).expect("a state folder")
}

fn build_root(s: &Sandbox, repo: &str) -> PathBuf {
    names::build_root(&s.environment(), &common_dir(s, repo)).expect("a build root")
}

/// A clone whose worktrees are idle at once.
fn clone(s: &Sandbox) -> String {
    let repo = s.cloned();
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    repo
}

/// `sweep --repository <repo> <args> --format json`, expecting `code`.
fn sweep(s: &Sandbox, repo: &str, args: &[&str], code: i32) -> Value {
    let args: Vec<&str> = ["sweep", "--repository", repo]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    s.run_json(&args, None, code)
}

/// The one item of `list` whose `key` is `value`.
fn find<'a>(list: &'a Value, key: &str, value: &str) -> &'a Value {
    list.as_array()
        .and_then(|items| items.iter().find(|item| item[key] == value))
        .unwrap_or_else(|| panic!("no {key} {value} in {list:#}"))
}

fn worktree<'a>(report: &'a Value, path: &str) -> &'a Value {
    find(&report["repositories"][0]["worktrees"], "path", path)
}

fn branch<'a>(report: &'a Value, name: &str) -> &'a Value {
    find(&report["repositories"][0]["branches"], "name", name)
}

fn codes(notes: &Value) -> Vec<&str> {
    notes
        .as_array()
        .map(|notes| notes.iter().filter_map(|n| n["code"].as_str()).collect())
        .unwrap_or_default()
}

fn backup(report: &Value) -> String {
    report["repositories"][0]["backup"]["path"]
        .as_str()
        .expect("a backup path")
        .to_owned()
}

/// `git <args>` in the sandbox, giving its output when it exits 0.
fn git_maybe(s: &Sandbox, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(&s.tmp)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// What `rev` names in the repository whose git directory is `git_dir`.
fn rev_parse(s: &Sandbox, git_dir: &str, rev: &str) -> Option<String> {
    git_maybe(
        s,
        &[
            "--git-dir",
            git_dir,
            "rev-parse",
            "--verify",
            "--quiet",
            rev,
        ],
    )
}

/// A detached worktree `<repo>.worktrees/<name>` with a commit of its own,
/// its folder deleted; its path and `HEAD`.
fn detached_gone(s: &Sandbox, repo: &str, name: &str) -> (String, String) {
    let path = join(&format!("{repo}.worktrees"), name);
    s.git(
        repo,
        &["worktree", "add", "--quiet", "--detach", &path, "HEAD"],
    );
    let path = real(&path);
    let head = s.commit(&path, &format!("{name}.txt"), &format!("{name}\n"));
    fs::remove_dir_all(&path).expect("the folder goes");
    (path, head)
}

/// Writes an executable hook.
fn hook(s: &Sandbox, path: &str, text: &str) {
    s.write(path, text);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("executable");
}

/// The repository's backup, made bare before the sweep, with `text` as its
/// `post-receive` hook, which runs after the sweep's push and before its
/// compare pass.
fn backup_with_hook(s: &Sandbox, repo: &str, text: &str) -> String {
    let report = s.run_json(&["status", "--repository", repo], None, 0);
    let backup = backup(&report);
    s.sh(&["git", "init", "--quiet", "--bare", &backup], None);
    hook(s, &join(&backup, "hooks/post-receive"), text);
    backup
}

/// `text` as one `sh` word.
fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

// The sandbox ------------------------------------------------------------------------------

#[test]
fn test_every_write_stays_inside_the_sandbox() {
    let s = Sandbox::new();
    for name in [
        "HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_CONFIG_HOME",
    ] {
        assert!(
            Path::new(&s.var(name)).starts_with(&s.tmp),
            "{name} is {}",
            s.var(name)
        );
    }
    let repo = clone(&s);
    detached_gone(&s, &repo, "gone");
    let report = sweep(&s, &repo, &[], 0);
    assert!(Path::new(&backup(&report)).starts_with(&s.tmp));
    assert!(exists(&backup(&report)));
    assert!(state(&s, &repo).starts_with(&s.tmp));
    assert!(state(&s, &repo).join(LAST_SWEEP_FILE).exists());
}

// How git prunes one record --------------------------------------------------------------

/// `git worktree remove` on a worktree whose folder is gone deletes its
/// record alone, and refuses a locked one: the sweep prunes one record at
/// a time through it, never with `git worktree prune`, which prunes every
/// missing folder at once.
#[test]
fn test_git_removes_one_missing_folders_record_and_refuses_a_locked_one() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let (one, _) = detached_gone(&s, &repo, "one");
    let (two, _) = detached_gone(&s, &repo, "two");
    let (locked, _) = detached_gone(&s, &repo, "locked");
    s.git(&repo, &["worktree", "lock", &locked]);
    s.git(&repo, &["worktree", "remove", &one]);
    let listed = s.listed(&repo);
    assert!(!listed.contains(&one), "{listed:?}");
    assert!(
        listed.contains(&two) && listed.contains(&locked),
        "{listed:?}"
    );
    assert_eq!(
        git_maybe(&s, &["-C", &repo, "worktree", "remove", &locked]),
        None
    );
    assert!(s.listed(&repo).contains(&locked));
}

// The archive before a prune ------------------------------------------------------------

#[test]
fn test_a_missing_folders_detached_head_is_archived_then_pruned() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let (gone, head) = detached_gone(&s, &repo, "gone");
    let report = sweep(&s, &repo, &[], 0);
    let pruned = worktree(&report, &gone);
    assert_eq!(
        (
            &pruned["action"],
            &pruned["outcome"],
            &pruned["archive"]["state"]
        ),
        (
            &Value::from("prune"),
            &Value::from("done"),
            &Value::from("verified")
        )
    );
    let reference = pruned["archive"]["ref"].as_str().expect("an archive ref");
    assert!(
        reference.starts_with("refs/archive/detached-head/gone/"),
        "{reference}"
    );
    assert_eq!(
        rev_parse(&s, &backup(&report), reference),
        Some(head.clone())
    );
    assert!(!s.listed(&repo).contains(&gone));
    assert_eq!(report["repositories"][0]["backup"]["verified"], 1);
}

#[test]
fn test_a_merged_branch_in_a_missing_folder_is_pruned_with_no_archive() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let landed = s.add_worktree(&repo, "landed");
    fs::remove_dir_all(&landed).expect("the folder goes");
    let report = sweep(&s, &repo, &[], 0);
    let pruned = worktree(&report, &landed);
    assert_eq!(
        (&pruned["action"], &pruned["outcome"], &pruned["archive"]),
        (&Value::from("prune"), &Value::from("done"), &Value::Null)
    );
    let deleted = branch(&report, "landed");
    assert_eq!(
        (&deleted["action"], &deleted["outcome"], &deleted["archive"]),
        (&Value::from("delete"), &Value::from("done"), &Value::Null)
    );
    assert!(!s.listed(&repo).contains(&landed));
    assert_eq!(
        rev_parse(&s, &join(&repo, ".git"), "refs/heads/landed"),
        None
    );
    assert!(!exists(&backup(&report)), "nothing needed the backup");
}

#[test]
fn test_a_failed_archive_keeps_its_worktree_registered_and_a_passed_siblings_is_pruned() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let (broken, _) = detached_gone(&s, &repo, "broken");
    let (sibling, sibling_head) = detached_gone(&s, &repo, "sibling");
    // The backup drops `broken`'s archive as soon as it is pushed, so its
    // compare fails while `sibling`'s passes.
    backup_with_hook(
        &s,
        &repo,
        "#!/bin/sh\n\
         while read -r old new ref; do\n\
         case \"$ref\" in refs/archive/detached-head/broken/*) git update-ref -d \"$ref\";; esac\n\
         done\n",
    );
    let report = sweep(&s, &repo, &[], 1);
    let kept = worktree(&report, &broken);
    assert_eq!(kept["action"], "keep");
    assert!(
        codes(&kept["reasons"]).contains(&"archive-failed"),
        "{kept:#}"
    );
    assert!(codes(&kept["attention"]).contains(&"archive-failed"));
    assert_eq!(kept["archive"]["state"], "failed");
    let pruned = worktree(&report, &sibling);
    assert_eq!(
        (&pruned["action"], &pruned["outcome"]),
        (&Value::from("prune"), &Value::from("done"))
    );
    let listed = s.listed(&repo);
    assert!(listed.contains(&broken), "{listed:?}");
    assert!(!listed.contains(&sibling), "{listed:?}");
    let reference = pruned["archive"]["ref"].as_str().expect("an archive ref");
    assert_eq!(
        rev_parse(&s, &backup(&report), reference),
        Some(sibling_head)
    );
    assert_eq!(report["totals"]["archives.failed"], 1);
}

/// The node's check: a sweep whose backup lacks an archived tip keeps
/// that branch and exits 1.
#[test]
fn test_a_sweep_whose_backup_lacks_an_archived_tip_keeps_that_branch_and_exits_1() {
    let s = Sandbox::new();
    let repo = clone(&s);
    s.git(&repo, &["checkout", "--quiet", "-b", "preview/solo"]);
    let tip = s.commit(&repo, "solo.txt", "only on preview/solo\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let written = s.run_json(
        &["archive", "write", "preview/solo", "--repository", &repo],
        None,
        0,
    );
    let backup = backup(&written);
    let blob = s.git(&repo, &["rev-parse", "preview/solo:solo.txt"]);
    let blob = blob.trim();
    fs::remove_file(join(
        &backup,
        &format!("objects/{}/{}", &blob[..2], &blob[2..]),
    ))
    .expect("the blob goes from the backup");
    let report = sweep(&s, &repo, &[], 1);
    let kept = branch(&report, "preview/solo");
    assert_eq!(kept["action"], "keep");
    assert!(
        codes(&kept["reasons"]).contains(&"archive-failed"),
        "{kept:#}"
    );
    assert_eq!(
        rev_parse(&s, &join(&repo, ".git"), "refs/heads/preview/solo"),
        Some(tip)
    );
}

#[test]
fn test_the_last_line_asks_for_a_backup_of_the_new_archives() {
    let s = Sandbox::new();
    let repo = clone(&s);
    detached_gone(&s, &repo, "gone");
    let ran = s.run(&["sweep", "--repository", &repo], None, &[]);
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    assert_eq!(
        ran.stdout.lines().last(),
        Some("1 new archive in ~/backups/git-archive/; back up ~/backups/")
    );
    let again = s.run(&["sweep", "--repository", &repo], None, &[]);
    assert!(!again.stdout.contains("new archive"), "{}", again.stdout);
}

// Orphaned build output --------------------------------------------------------------------

/// `wt/<id>/` for the worktree at `path`, holding a file, and the
/// back-pointer `pointer` unless it is None.
fn output(s: &Sandbox, repo: &str, path: &str, pointer: Option<&[u8]>) -> PathBuf {
    let layout = Layout {
        root: build_root(s, repo),
        worktree: PathBuf::from(path),
        main: false,
    };
    let dir = layout.dir();
    s.write(&dir.join("target/out.o").to_string_lossy(), "built\n");
    if let Some(bytes) = pointer {
        fs::write(dir.join(WORKTREE_FILE), bytes).expect("a back-pointer");
    }
    dir
}

#[test]
fn test_orphaned_build_output_is_listed_by_a_dry_run_then_deleted_and_nothing_else() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let kept = s.add_worktree(&repo, "kept");
    s.commit(&kept, "kept.txt", "kept\n");
    let old = s.add_worktree(&repo, "old");
    s.git(&repo, &["worktree", "remove", &old]);
    let owned = output(&s, &repo, &kept, Some(format!("{kept}\n").as_bytes()));
    let orphan = output(&s, &repo, &old, Some(format!("{old}\n").as_bytes()));
    let unclaimed = output(&s, &repo, &join(&s.tmp, "stray"), None);
    let cut = output(
        &s,
        &repo,
        &join(&s.tmp, "cut"),
        Some(join(&s.tmp, "elsewhere").as_bytes()),
    );
    let entry = |report: &Value, dir: &Path| {
        report["repositories"][0]["build_output"]
            .as_array()
            .expect("build output")
            .iter()
            .find(|b| b["path"] == dir.to_string_lossy().as_ref())
            .cloned()
    };

    let planned = sweep(&s, &repo, &["--dry-run"], 0);
    let listed = entry(&planned, &orphan).expect("the orphan is listed");
    assert_eq!(
        (&listed["action"], &listed["outcome"], &listed["worktree"]),
        (
            &Value::from("delete"),
            &Value::from("planned"),
            &Value::from(old.as_str())
        )
    );
    assert!(orphan.exists(), "a dry run deletes nothing");
    let text = s.run(&["sweep", "--dry-run", "--repository", &repo], None, &[]);
    assert!(
        text.stdout.contains(&orphan.to_string_lossy().into_owned()),
        "{}",
        text.stdout
    );

    let swept = sweep(&s, &repo, &[], 0);
    let deleted = entry(&swept, &orphan).expect("the orphan is reported");
    assert_eq!(
        (&deleted["action"], &deleted["outcome"]),
        (&Value::from("delete"), &Value::from("done"))
    );
    assert!(!orphan.exists());
    assert!(owned.join("target/out.o").exists() && owned.join(WORKTREE_FILE).exists());
    assert_eq!(
        entry(&swept, &owned),
        None,
        "a listed worktree's is no orphan"
    );
    for dir in [&unclaimed, &cut] {
        let reported = entry(&swept, dir).expect("reported");
        assert_eq!(
            (
                &reported["action"],
                &reported["outcome"],
                &reported["worktree"]
            ),
            (&Value::from("keep"), &Value::from("none"), &Value::Null),
            "{}",
            dir.display()
        );
        assert!(dir.join("target/out.o").exists(), "{}", dir.display());
    }
}

#[test]
fn test_a_removed_worktrees_build_output_goes_in_the_same_sweep() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let landed = s.add_worktree(&repo, "landed");
    let dir = output(&s, &repo, &landed, Some(format!("{landed}\n").as_bytes()));
    let planned = sweep(&s, &repo, &["--dry-run"], 0);
    assert_eq!(
        planned["repositories"][0]["build_output"][0]["outcome"],
        "planned"
    );
    let swept = sweep(&s, &repo, &[], 0);
    assert_eq!(worktree(&swept, &landed)["outcome"], "done");
    assert_eq!(
        swept["repositories"][0]["build_output"][0]["outcome"],
        "done"
    );
    assert!(!dir.exists());
}

// A dry run ----------------------------------------------------------------------------------

/// Every ref, the worktree list, and every file's and folder's
/// modification time and size under the sandbox, the state folder, the
/// build root and the backup among them.
fn snapshot(s: &Sandbox, repo: &str) -> (String, String, BTreeMap<PathBuf, (u128, u64)>) {
    fn walk(path: &Path, into: &mut BTreeMap<PathBuf, (u128, u64)>) {
        let meta = fs::symlink_metadata(path).expect("metadata");
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        into.insert(path.to_owned(), (modified, meta.len()));
        if meta.is_dir() {
            fs::read_dir(path)
                .expect("a folder")
                .for_each(|entry| walk(&entry.expect("an entry").path(), into));
        }
    }
    let refs = s.git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname) %(symref)",
        ],
    );
    let list = s.git(
        repo,
        &["--no-optional-locks", "worktree", "list", "--porcelain"],
    );
    let mut times = BTreeMap::new();
    walk(Path::new(&s.tmp), &mut times);
    (refs, list, times)
}

#[test]
fn test_a_dry_run_changes_nothing() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let landed = s.add_worktree(&repo, "landed");
    let feat = s.add_worktree(&repo, "feat");
    s.commit(&feat, "feat.txt", "feat\n");
    s.write(&join(&feat, "notes.txt"), "uncommitted\n");
    detached_gone(&s, &repo, "gone");
    s.git(&repo, &["checkout", "--quiet", "-b", "preview/try"]);
    s.commit(&repo, "try.txt", "try\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let old = s.add_worktree(&repo, "old");
    s.git(&repo, &["worktree", "remove", &old]);
    output(&s, &repo, &old, Some(format!("{old}\n").as_bytes()));
    // A branch deleted by hand, logged, whose commit nothing else holds.
    let lost = s.git(
        &repo,
        &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "lost"],
    );
    let lost = lost.trim();
    let zero = "0".repeat(40);
    s.write(
        &state(&s, &repo).join("events.jsonl").to_string_lossy(),
        &format!(
            "{{\"time\":\"2026-11-19T09:00:00Z\",\"pid\":1,\"event\":\"ref\",\"ref\":\"refs/heads/lost\",\"old\":\"{lost}\",\"new\":\"{zero}\"}}\n"
        ),
    );
    // The same bytes at another time: a run that refreshed the index
    // would write it.
    for worktree in [&feat, &landed] {
        utime(&join(worktree, ".gitignore"), 1_000_000_000.0);
    }
    let before = snapshot(&s, &repo);
    let planned = sweep(&s, &repo, &["--dry-run"], 0);
    assert_eq!(snapshot(&s, &repo), before, "the first dry run wrote");
    let repository = &planned["repositories"][0];
    assert_eq!(
        repository["totals"]["archives.planned"], 3,
        "{repository:#}"
    );
    assert_eq!(repository["rescues"][0]["outcome"], "planned");
    assert_eq!(repository["build_output"][0]["outcome"], "planned");
    for format in FORMATS {
        for cwd in [&repo, &feat] {
            for args in [
                vec!["sweep", "--dry-run", "--format", format],
                vec!["sweep", "--dry-run", "--unattended", "--format", format],
            ] {
                let ran = s.run(&args, Some(cwd), &[]);
                assert_eq!(ran.code, 0, "{args:?}: {}", ran.stderr);
                assert_eq!(snapshot(&s, &repo), before, "{args:?} in {cwd} wrote");
            }
        }
    }
    assert!(!state(&s, &repo).join(SWEEP_LOCK_FILE).exists());
    assert!(!state(&s, &repo).join(LAST_SWEEP_FILE).exists());
}

// --unattended ---------------------------------------------------------------------------------

#[test]
fn test_unattended_acts_only_where_the_repository_says_sweep() {
    for (value, acts) in [
        (None, false),
        (Some("report"), false),
        (Some("sweep"), true),
        (Some("Sweep"), true),
        (Some("junk"), false),
    ] {
        let s = Sandbox::new();
        let repo = clone(&s);
        if let Some(value) = value {
            s.git(&repo, &["config", "worktreeManager.unattended", value]);
        }
        let landed = s.add_worktree(&repo, "landed");
        let report = sweep(&s, &repo, &["--unattended"], 0);
        let outcome = if acts { "done" } else { "planned" };
        assert_eq!(worktree(&report, &landed)["outcome"], outcome, "{value:?}");
        assert_eq!(exists(&landed), !acts, "{value:?}");
        assert_eq!(report["dry_run"], !acts, "{value:?}");
        let saved: Value = serde_json::from_str(&sandbox::read(
            &state(&s, &repo).join(LAST_SWEEP_FILE).to_string_lossy(),
        ))
        .expect("the report written");
        assert_eq!(saved["repositories"], report["repositories"], "{value:?}");
    }
}

// What moved between the plan and the act -------------------------------------------------------

#[test]
fn test_a_worktree_made_dirty_or_a_branch_moved_after_the_plan_is_kept_and_failed() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let dirty = s.add_worktree(&repo, "dirty");
    s.git(&repo, &["branch", "moved"]);
    // A branch to archive, so the sweep pushes into the backup.
    s.git(&repo, &["checkout", "--quiet", "-b", "preview/try"]);
    s.commit(&repo, "try.txt", "try\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let elsewhere = s.git(
        &repo,
        &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "moved on"],
    );
    let elsewhere = elsewhere.trim();
    let built = output(&s, &repo, &dirty, Some(format!("{dirty}\n").as_bytes()));
    backup_with_hook(
        &s,
        &repo,
        &format!(
            "#!/bin/sh\n\
             printf 'new\\n' > {}\n\
             git --git-dir={} update-ref refs/heads/moved {elsewhere}\n",
            quoted(&join(&dirty, "new.txt")),
            quoted(&join(&repo, ".git")),
        ),
    );
    let report = sweep(&s, &repo, &[], 1);
    let kept = worktree(&report, &dirty);
    assert_eq!(kept["outcome"], "failed", "{kept:#}");
    assert!(
        kept["error"]
            .as_str()
            .is_some_and(|e| e.contains("changed since the plan")),
        "{kept:#}"
    );
    assert!(exists(&join(&dirty, "new.txt")));
    // Its build output was planned to go with it, and stays with it.
    assert!(built.join("target/out.o").exists());
    let output_report = report["repositories"][0]["build_output"]
        .as_array()
        .and_then(|b| {
            b.iter()
                .find(|b| b["path"] == built.to_string_lossy().as_ref())
        })
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(output_report["action"], "keep", "{output_report:#}");
    let moved = branch(&report, "moved");
    assert_eq!(moved["outcome"], "failed", "{moved:#}");
    assert_eq!(
        rev_parse(&s, &join(&repo, ".git"), "refs/heads/moved").as_deref(),
        Some(elsewhere)
    );
    assert_eq!(branch(&report, "preview/try")["outcome"], "done");
}

/// The same, with a sweep left waiting, so a second round runs: it plans
/// the dirty worktree kept, and its failure stays in the report printed
/// and written, and the sweep exits 1; it deletes the moved branch, which
/// ends that failure.
#[test]
fn test_a_failure_in_one_round_outlasts_a_later_round_that_keeps_the_item() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let dirty = s.add_worktree(&repo, "dirty");
    s.git(&repo, &["branch", "moved"]);
    s.git(&repo, &["checkout", "--quiet", "-b", "preview/try"]);
    s.commit(&repo, "try.txt", "try\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    let elsewhere = s.git(
        &repo,
        &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "moved on"],
    );
    let elsewhere = elsewhere.trim();
    let pending = state(&s, &repo).join(SWEEP_PENDING_FILE);
    backup_with_hook(
        &s,
        &repo,
        &format!(
            "#!/bin/sh\n\
             printf 'new\\n' > {}\n\
             git --git-dir={} update-ref refs/heads/moved {elsewhere}\n\
             : > {}\n",
            quoted(&join(&dirty, "new.txt")),
            quoted(&join(&repo, ".git")),
            quoted(&pending.to_string_lossy()),
        ),
    );
    let printed = sweep(&s, &repo, &[], 1);
    assert!(!pending.exists(), "a second round ran");
    let written: Value = serde_json::from_str(&sandbox::read(
        &state(&s, &repo).join(LAST_SWEEP_FILE).to_string_lossy(),
    ))
    .expect("the report written");
    for report in [&printed, &written] {
        assert_eq!(worktree(report, &dirty)["outcome"], "failed", "{report:#}");
        assert_eq!(branch(report, "moved")["outcome"], "done", "{report:#}");
        assert_eq!(report["totals"]["failed"], 1, "{report:#}");
    }
}

// Symbolic branches ------------------------------------------------------------------------------

fn symbolic_target(s: &Sandbox, repo: &str, name: &str) -> Option<String> {
    git_maybe(s, &["-C", repo, "symbolic-ref", "--quiet", name])
}

/// `git update-ref --no-deref -d <ref> <old>` deletes a symbolic ref
/// itself, never its target, and still refuses an `<old>` that isn't the
/// value it holds.
#[test]
fn test_git_deletes_a_symbolic_ref_itself_under_no_deref_and_tests_the_old_value() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let main = s.git(&repo, &["rev-parse", "main"]).trim().to_owned();
    let other = s
        .git(
            &repo,
            &["commit-tree", "HEAD^{tree}", "-p", "HEAD", "-m", "other"],
        )
        .trim()
        .to_owned();
    s.git(&repo, &["branch", "plain"]);
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/master", "refs/heads/main"],
    );
    let delete = |reference: &str, old: &str| {
        git_maybe(
            &s,
            &[
                "-C",
                &repo,
                "update-ref",
                "--no-deref",
                "-d",
                reference,
                old,
            ],
        )
    };
    let git_dir = join(&repo, ".git");
    assert_eq!(delete("refs/heads/plain", &other), None);
    assert!(rev_parse(&s, &git_dir, "refs/heads/plain").is_some());
    assert!(delete("refs/heads/plain", &main).is_some());
    assert_eq!(rev_parse(&s, &git_dir, "refs/heads/plain"), None);
    assert_eq!(delete("refs/heads/master", &other), None);
    assert!(symbolic_target(&s, &repo, "refs/heads/master").is_some());
    assert!(delete("refs/heads/master", &main).is_some());
    assert_eq!(symbolic_target(&s, &repo, "refs/heads/master"), None);
    assert_eq!(rev_parse(&s, &git_dir, "refs/heads/main"), Some(main));
}

#[test]
fn test_a_symbolic_branch_and_its_target_both_stay() {
    let s = Sandbox::new();
    let repo = clone(&s);
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/master", "refs/heads/main"],
    );
    s.git(&repo, &["checkout", "--quiet", "-b", "feature"]);
    let feature = s.commit(&repo, "feature.txt", "feature\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/preview/x", "refs/heads/feature"],
    );
    let main = s.git(&repo, &["rev-parse", "main"]).trim().to_owned();
    let report = sweep(&s, &repo, &[], 0);
    let names: Vec<&str> = report["repositories"][0]["branches"]
        .as_array()
        .expect("branches")
        .iter()
        .filter_map(|b| b["name"].as_str())
        .collect();
    assert!(
        !names.contains(&"master") && !names.contains(&"preview/x"),
        "{names:?}"
    );
    let git_dir = join(&repo, ".git");
    assert_eq!(rev_parse(&s, &git_dir, "refs/heads/main"), Some(main));
    assert_eq!(rev_parse(&s, &git_dir, "refs/heads/feature"), Some(feature));
    assert_eq!(
        symbolic_target(&s, &repo, "refs/heads/master").as_deref(),
        Some("refs/heads/main")
    );
    assert_eq!(
        symbolic_target(&s, &repo, "refs/heads/preview/x").as_deref(),
        Some("refs/heads/feature")
    );
}

/// A landed branch checked out through an alias in a worktree that stays
/// has that worktree as its holder, so it stays too.
#[test]
fn test_a_branch_checked_out_through_an_alias_stays_with_its_worktree() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let path = s.add_worktree(&repo, "feature");
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/alias", "refs/heads/feature"],
    );
    s.git(&path, &["symbolic-ref", "HEAD", "refs/heads/alias"]);
    s.write(&join(&path, "notes.txt"), "uncommitted\n");
    let feature = s.git(&repo, &["rev-parse", "feature"]).trim().to_owned();
    let report = sweep(&s, &repo, &[], 0);
    assert_eq!(worktree(&report, &path)["action"], "keep", "{report:#}");
    assert_eq!(branch(&report, "feature")["action"], "keep", "{report:#}");
    assert_eq!(
        rev_parse(&s, &join(&repo, ".git"), "refs/heads/feature"),
        Some(feature)
    );
}

/// The dry run's report for a worktree on `feature` whose `HEAD` is then
/// made the symbolic ref `top`, through `links` (each an alias and what
/// it names), which git can't resolve, with the worktree's path; the
/// worktree is kept.
fn unresolved_head(links: &[(&str, &str)], top: &str) -> (Value, String) {
    let s = Sandbox::new();
    let repo = clone(&s);
    let path = s.add_worktree(&repo, "feature");
    for (alias, target) in links {
        s.git(
            &repo,
            &[
                "symbolic-ref",
                &format!("refs/heads/{alias}"),
                &format!("refs/heads/{target}"),
            ],
        );
    }
    s.git(
        &path,
        &["symbolic-ref", "HEAD", &format!("refs/heads/{top}")],
    );
    assert!(
        git_maybe(
            &s,
            &["-C", &path, "rev-parse", "--verify", "--quiet", "HEAD"]
        )
        .is_none(),
        "git can't resolve its HEAD"
    );
    let report = sweep(&s, &repo, &["--dry-run"], 0);
    assert_eq!(worktree(&report, &path)["action"], "keep", "{report:#}");
    (report, path)
}

/// Git reads at most 5 refs for one name, `HEAD` among them, so it can't
/// resolve `HEAD` through 4 aliases; the worktree stays, holding the
/// landed branch they end at.
#[test]
fn test_a_worktree_on_a_chain_too_long_for_git_keeps_its_branch() {
    let (report, _) = unresolved_head(
        &[("a1", "feature"), ("a2", "a1"), ("a3", "a2"), ("a4", "a3")],
        "a4",
    );
    assert_eq!(branch(&report, "feature")["action"], "keep", "{report:#}");
}

/// A chain that loops ends at no branch: the worktree stays, on none.
#[test]
fn test_a_worktree_on_a_looping_chain_stays_on_no_branch() {
    let (report, path) = unresolved_head(&[("loop-a", "loop-b"), ("loop-b", "loop-a")], "loop-a");
    assert_eq!(
        worktree(&report, &path)["branch"],
        Value::Null,
        "{report:#}"
    );
}

// Build output, more ------------------------------------------------------------------------------

fn build_entry(report: &Value, dir: &Path) -> Option<Value> {
    report["repositories"][0]["build_output"]
        .as_array()
        .and_then(|b| {
            b.iter()
                .find(|b| b["path"] == dir.to_string_lossy().as_ref())
        })
        .cloned()
}

/// On a volume that ignores case, a back-pointer that names a listed
/// worktree in other case is that worktree's, though its id differs.
#[test]
fn test_a_back_pointer_naming_a_listed_worktree_in_other_case_is_kept() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let kept = s.add_worktree(&repo, "kept");
    s.commit(&kept, "kept.txt", "kept\n");
    let other_case = join(&format!("{repo}.worktrees"), "KEPT");
    if !exists(&other_case) {
        // A volume that tells case apart: no such folder to name.
        return;
    }
    let dir = output(
        &s,
        &repo,
        &other_case,
        Some(format!("{other_case}\n").as_bytes()),
    );
    let report = sweep(&s, &repo, &[], 0);
    assert!(dir.join("target/out.o").exists(), "{report:#}");
    assert_eq!(build_entry(&report, &dir), None);
}

#[test]
fn test_a_relative_back_pointer_is_reported_not_deleted() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let dir = output(&s, &repo, &join(&s.tmp, "rel"), Some(b"gone/away\n"));
    let report = sweep(&s, &repo, &[], 0);
    let entry = build_entry(&report, &dir).expect("reported");
    assert_eq!(
        (&entry["action"], &entry["worktree"]),
        (&Value::from("keep"), &Value::Null),
        "{entry:#}"
    );
    assert!(dir.join("target/out.o").exists());
}

#[test]
fn test_an_unreadable_back_pointer_is_reported_and_the_rest_goes_on() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let odd = output(&s, &repo, &join(&s.tmp, "odd"), None);
    fs::create_dir(odd.join(WORKTREE_FILE)).expect("a folder named WORKTREE");
    let old = s.add_worktree(&repo, "old");
    s.git(&repo, &["worktree", "remove", &old]);
    let orphan = output(&s, &repo, &old, Some(format!("{old}\n").as_bytes()));
    // Nothing fails in a dry run, which says why all the same; the sweep
    // that acts reports it kept and failed.
    for (args, code, outcome) in [(vec!["--dry-run"], 0, "none"), (vec![], 1, "failed")] {
        let report = sweep(&s, &repo, &args, code);
        assert_eq!(report["repositories"][0]["problem"], Value::Null);
        let entry = build_entry(&report, &odd).expect("reported");
        assert_eq!(
            (&entry["action"], &entry["outcome"]),
            (&Value::from("keep"), &Value::from(outcome)),
            "{entry:#}"
        );
        assert!(entry["error"].as_str().is_some_and(|e| !e.is_empty()));
    }
    let status = s.run_json(&["status", "--repository", &repo], None, 0);
    assert!(build_entry(&status, &odd).is_some());
    assert!(odd.join("target/out.o").exists());
    assert!(!orphan.exists());
}

/// A listed worktree whose folder can't be identified, here one whose
/// path runs through a file, might own any `wt/<id>`: no build output
/// is deleted, each is reported with why, and the rest is swept.
#[test]
fn test_a_listed_worktree_that_cant_be_identified_stops_every_build_output_delete() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let blocker = join(&s.tmp, "blocker");
    let blocked = join(&blocker, "wt");
    s.git(
        &repo,
        &["worktree", "add", "--quiet", "--detach", &blocked, "HEAD"],
    );
    let blocked = real(&blocked);
    s.git(&repo, &["worktree", "lock", &blocked]);
    fs::remove_dir_all(&blocker).expect("the folder goes");
    s.write(&blocker, "a file where a folder was\n");
    let old = s.add_worktree(&repo, "old");
    s.git(&repo, &["worktree", "remove", &old]);
    let orphan = output(&s, &repo, &old, Some(format!("{old}\n").as_bytes()));
    let landed = s.add_worktree(&repo, "landed");
    let report = sweep(&s, &repo, &[], 1);
    assert_eq!(
        report["repositories"][0]["problem"],
        Value::Null,
        "{report:#}"
    );
    let entry = build_entry(&report, &orphan).expect("reported");
    assert_eq!(
        (&entry["action"], &entry["outcome"]),
        (&Value::from("keep"), &Value::from("failed")),
        "{entry:#}"
    );
    assert!(
        entry["error"]
            .as_str()
            .is_some_and(|e| e.contains("can't be identified")),
        "{entry:#}"
    );
    assert!(orphan.join("target/out.o").exists());
    assert_eq!(worktree(&report, &landed)["outcome"], "done");
}

// A HEAD that can't be read ----------------------------------------------------------------------

/// The administrative folder git keeps for the worktree at `path`.
fn admin_dir(s: &Sandbox, path: &str) -> String {
    let list = s.git(path, &["rev-parse", "--path-format=absolute", "--git-dir"]);
    list.trim().to_owned()
}

#[test]
fn test_a_missing_folder_whose_head_cant_be_read_stays_registered() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let path = join(&format!("{repo}.worktrees"), "unreadable");
    s.git(
        &repo,
        &["worktree", "add", "--quiet", "--detach", &path, "HEAD"],
    );
    let path = real(&path);
    s.commit(&path, "mine.txt", "mine\n");
    let admin = admin_dir(&s, &path);
    fs::remove_dir_all(&path).expect("the folder goes");
    fs::write(join(&admin, "HEAD"), "not a commit\n").expect("a broken HEAD");
    let report = sweep(&s, &repo, &[], 0);
    let kept = worktree(&report, &path);
    assert_eq!(kept["action"], "keep", "{kept:#}");
    assert!(codes(&kept["reasons"]).contains(&"broken"), "{kept:#}");
    assert!(s.listed(&repo).contains(&path));
}

#[test]
fn test_a_missing_folder_on_a_branch_with_no_commit_is_pruned() {
    let s = Sandbox::new();
    let repo = clone(&s);
    let path = s.add_worktree(&repo, "unborn");
    let admin = admin_dir(&s, &path);
    fs::remove_dir_all(&path).expect("the folder goes");
    fs::write(join(&admin, "HEAD"), "ref: refs/heads/never-made\n").expect("an unborn HEAD");
    let report = sweep(&s, &repo, &[], 0);
    let pruned = worktree(&report, &path);
    assert_eq!(
        (&pruned["action"], &pruned["outcome"]),
        (&Value::from("prune"), &Value::from("done")),
        "{pruned:#}"
    );
    assert!(!s.listed(&repo).contains(&path));
}

// Outside a repository ---------------------------------------------------------------------------

#[test]
fn test_an_acting_sweep_outside_a_repository_prints_nothing_and_exits_1() {
    let s = Sandbox::new();
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    for format in FORMATS {
        for args in [
            vec!["sweep", "--format", format],
            vec!["sweep", "--repository", &plain, "--format", format],
        ] {
            let ran = s.run(&args, Some(&plain), &[]);
            assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{args:?}");
            assert!(
                ran.stderr.contains("not in a git repository"),
                "{args:?}: {}",
                ran.stderr
            );
        }
    }
}
