//! Nothing is lost: over several repositories holding every kind of
//! worktree and branch, each commit a branch or a worktree held before a
//! sweep is still reachable after it, in the repository or in its backup,
//! and each uncommitted or kept file is still there.

mod sandbox;

use std::collections::BTreeSet;
use std::fs;
use std::process::{Command, Stdio};

use sandbox::{Sandbox, exists, join, real};
use serde_json::Value;

/// A commit on `parent` in the repository at `git_dir`, that no ref holds.
fn loose(s: &Sandbox, git_dir: &str, parent: &str, message: &str) -> String {
    s.sh(
        &[
            "git",
            "--git-dir",
            git_dir,
            "commit-tree",
            &format!("{parent}^{{tree}}"),
            "-p",
            parent,
            "-m",
            message,
        ],
        None,
    )
    .trim()
    .to_owned()
}

/// Every branch tip and worktree `HEAD` of the repository at `repo`.
fn held(s: &Sandbox, repo: &str) -> BTreeSet<String> {
    let tips = s.git(
        repo,
        &["for-each-ref", "--format=%(objectname)", "refs/heads"],
    );
    let list = s.git(repo, &["worktree", "list", "--porcelain"]);
    tips.lines()
        .map(str::to_owned)
        .chain(
            list.lines()
                .filter_map(|line| line.strip_prefix("HEAD "))
                .map(str::to_owned),
        )
        .collect()
}

/// Whether some ref of the repository at `git_dir` reaches `commit`.
fn reachable(s: &Sandbox, git_dir: &str, commit: &str) -> bool {
    let output = Command::new("git")
        .args(["--git-dir", git_dir, "rev-list", commit, "--not", "--all"])
        .current_dir(&s.tmp)
        .env_clear()
        .envs(s.env.iter().map(|(n, v)| (n.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .output()
        .expect("git runs");
    output.status.success() && output.stdout.is_empty()
}

fn git_dir(s: &Sandbox, repo: &str) -> String {
    real(
        s.git(
            repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .trim(),
    )
}

/// A worktree `<repo>.worktrees/<name>`, detached, with a commit of its
/// own; its path.
fn detached(s: &Sandbox, repo: &str, name: &str) -> String {
    let path = join(&format!("{repo}.worktrees"), name);
    s.git(
        repo,
        &["worktree", "add", "--quiet", "--detach", &path, "HEAD"],
    );
    let path = real(&path);
    s.commit(&path, &format!("{name}.txt"), &format!("{name}\n"));
    path
}

/// The clone, with every kind of worktree and branch; the files that must
/// survive, and the worktree that must go.
fn app(s: &Sandbox) -> (String, Vec<String>, String) {
    let repo = s.cloned();
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let remote = join(&s.tmp, "repo.remote.git");
    let landed = s.add_worktree(&repo, "landed");

    let unmerged = s.add_worktree(&repo, "unmerged");
    s.commit(&unmerged, "unmerged.txt", "unmerged\n");

    let diverged = s.add_worktree(&repo, "diverged");
    let base = s.commit(&diverged, "diverged.txt", "base\n");
    s.git(
        &diverged,
        &["push", "--quiet", "--set-upstream", "origin", "diverged"],
    );
    s.commit(&diverged, "diverged.txt", "ours\n");
    let theirs = loose(s, &remote, &base, "theirs");
    s.git(&remote, &["update-ref", "refs/heads/diverged", &theirs]);
    s.git(&repo, &["fetch", "--quiet", "origin"]);

    s.git(&repo, &["checkout", "--quiet", "-b", "upstream-gone"]);
    s.commit(&repo, "gone.txt", "gone\n");
    s.git(
        &repo,
        &[
            "push",
            "--quiet",
            "--set-upstream",
            "origin",
            "upstream-gone",
        ],
    );
    s.git(&repo, &["checkout", "--quiet", "main"]);
    s.git(&remote, &["update-ref", "-d", "refs/heads/upstream-gone"]);
    s.git(&repo, &["fetch", "--quiet", "--prune", "origin"]);

    s.git(&repo, &["checkout", "--quiet", "-b", "preview/try"]);
    s.commit(&repo, "try.txt", "try\n");
    s.git(&repo, &["checkout", "--quiet", "main"]);

    let gone = detached(s, &repo, "gone");
    fs::remove_dir_all(&gone).expect("the folder goes");
    let locked = detached(s, &repo, "locked");
    s.git(&repo, &["worktree", "lock", &locked]);
    fs::remove_dir_all(&locked).expect("the folder goes");

    let dirty = s.add_worktree(&repo, "dirty");
    let notes = join(&dirty, "notes.txt");
    s.write(&notes, "uncommitted\n");
    let keeps = s.add_worktree(&repo, "keeps");
    let kept = join(&keeps, "settings.local.json");
    s.write(&kept, "{\"mine\": true}\n");
    (repo, vec![notes, kept], landed)
}

/// A bare repository with a worktree of its own work and a landed one.
fn bare(s: &Sandbox) -> String {
    let bare = join(&s.tmp, "bare.git");
    s.sh(
        &[
            "git",
            "clone",
            "--quiet",
            "--bare",
            &join(&s.tmp, "repo.remote.git"),
            &bare,
        ],
        None,
    );
    s.git(&bare, &["config", "worktreeManager.idle", "0s"]);
    let own = join(&s.tmp, "bare.worktrees/own");
    s.git(
        &bare,
        &["worktree", "add", "--quiet", "-b", "own", &own, "main"],
    );
    s.commit(&own, "own.txt", "own\n");
    let landed = join(&s.tmp, "bare.worktrees/landed");
    s.git(
        &bare,
        &[
            "worktree", "add", "--quiet", "-b", "landed", &landed, "main",
        ],
    );
    real(&bare)
}

/// A repository with no remote, so no default branch.
fn plain(s: &Sandbox) -> String {
    let repo = s.repo_named("plain", &[]);
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let own = s.add_worktree(&repo, "own");
    s.commit(&own, "own.txt", "own\n");
    s.add_worktree(&repo, "landed");
    let gone = detached(s, &repo, "gone");
    fs::remove_dir_all(&gone).expect("the folder goes");
    repo
}

#[test]
fn test_nothing_is_lost_over_several_repositories() {
    let s = Sandbox::new();
    let (app, files, landed) = app(&s);
    let repositories = [app.clone(), bare(&s), plain(&s)];
    let before: Vec<(String, BTreeSet<String>)> = repositories
        .iter()
        .map(|repo| (git_dir(&s, repo), held(&s, repo)))
        .collect();
    let manifest = join(&s.tmp, "repositories.json");
    s.write(
        &manifest,
        &Value::Array(
            repositories
                .iter()
                .map(|path| serde_json::json!({ "path": path }))
                .collect(),
        )
        .to_string(),
    );
    let ran = s.run(
        &["sweep", "--repositories", &manifest, "--format", "json"],
        None,
        &[],
    );
    let report: Value = serde_json::from_str(&ran.stdout).expect("JSON");
    assert_eq!(ran.code, 0, "{}{report:#}", ran.stderr);
    assert!(!exists(&landed), "the sweep acted");
    let app_report = &report["repositories"][0];
    let done = |kind: &str, key: &str| -> BTreeSet<String> {
        app_report[kind]
            .as_array()
            .expect("items")
            .iter()
            .filter(|item| item["outcome"] == "done")
            .filter_map(|item| item[key].as_str())
            .map(str::to_owned)
            .collect()
    };
    assert!(
        done("worktrees", "path").contains(&join(&format!("{app}.worktrees"), "gone")),
        "{app_report:#}"
    );
    assert!(
        ["landed", "preview/try"]
            .iter()
            .all(|b| done("branches", "name").contains(*b)),
        "{app_report:#}"
    );
    let reported = report["repositories"].as_array().expect("repositories");
    assert_eq!(reported.len(), repositories.len());
    for ((git_dir, commits), repository) in before.iter().zip(reported) {
        let backup = repository["backup"]["path"].as_str().unwrap_or_default();
        let lost: Vec<&String> = commits
            .iter()
            .filter(|commit| {
                !reachable(&s, git_dir, commit)
                    && !(exists(backup) && reachable(&s, backup, commit))
            })
            .collect();
        assert!(lost.is_empty(), "{git_dir} lost {lost:?}: {repository:#}");
    }
    for file in &files {
        assert!(exists(file), "{file}");
    }
}
