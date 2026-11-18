//! The reading edge and `facts`, over sandbox repositories where gitoxide
//! and git could disagree; every read is checked against git's answers.

mod sandbox;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use git_worktree_manager::backup;
use git_worktree_manager::lock::LockForm;
use git_worktree_manager::model::facts::{BranchFacts, RepositoryFacts, WorktreeFacts};
use git_worktree_manager::model::ids::DefaultSource;
use git_worktree_manager::model::settings::{Settings, Unattended};
use git_worktree_manager::read;
use git_worktree_manager::system::Real;
use git_worktree_manager::targets::Target;
use jiff::SignedDuration;
use sandbox::{Sandbox, exists, join, real};
use serde::Deserialize;

/// A pid no process has: above every system's limit.
const NO_SUCH_PID: u32 = 2_147_483_646;

#[derive(Deserialize)]
struct Printed {
    facts: RepositoryFacts,
    settings: Settings,
}

fn facts(s: &Sandbox, checkout: &str, more: &[&str]) -> Printed {
    let args: Vec<&str> = ["facts", "--repository", checkout]
        .into_iter()
        .chain(more.iter().copied())
        .collect();
    serde_json::from_value(s.run_json(&args, None, 0)).expect("the facts")
}

fn worktree<'a>(facts: &'a RepositoryFacts, path: &str) -> &'a WorktreeFacts {
    facts
        .worktrees
        .iter()
        .find(|w| w.path == Path::new(path))
        .unwrap_or_else(|| panic!("no worktree {path} in {:#?}", facts.worktrees))
}

fn branch<'a>(facts: &'a RepositoryFacts, name: &str) -> &'a BranchFacts {
    facts
        .branches
        .iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("no branch {name}"))
}

fn paths(items: &[&str]) -> Vec<PathBuf> {
    items.iter().map(PathBuf::from).collect()
}

/// `git <args>` in `repo`, split on NUL, without its empty last part.
fn git_z(s: &Sandbox, repo: &str, args: &[&str]) -> Vec<String> {
    let out = s.git(repo, args);
    let mut parts: Vec<String> = out.split('\0').map(str::to_owned).collect();
    if parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

fn lock(s: &Sandbox, repo: &str, path: &str, reason: Option<&str>) {
    let args: Vec<&str> = ["worktree", "lock"]
        .into_iter()
        .chain(reason.map(|r| ["--reason", r]).into_iter().flatten())
        .chain([path])
        .collect();
    s.git(repo, &args);
}

// Agreeing with git -------------------------------------------------------------

/// Every fact git can state about the repository at `repo`, checked
/// against `facts`: worktrees, branches and upstreams, ancestry, content
/// landed, status and remote-tracking refs.
fn assert_agrees_with_git(s: &Sandbox, repo: &str, facts: &RepositoryFacts) {
    assert_worktrees_agree(s, repo, facts);
    assert_branches_agree(s, repo, facts);
    let remote: BTreeSet<(String, String)> = s
        .git(
            repo,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/remotes",
            ],
        )
        .lines()
        .filter(|line| !line.split(' ').next().unwrap_or("").ends_with("/HEAD"))
        .map(|line| {
            let (name, oid) = line.split_once(' ').expect("a ref");
            (name.to_owned(), oid.to_owned())
        })
        .collect();
    let read: BTreeSet<(String, String)> = facts
        .remote_refs
        .iter()
        .map(|r| (r.name.clone(), r.oid.to_string()))
        .collect();
    assert_eq!(read, remote, "remote-tracking refs");
}

fn assert_worktrees_agree(s: &Sandbox, repo: &str, facts: &RepositoryFacts) {
    let listed = s.git(repo, &["worktree", "list", "--porcelain", "-z"]);
    let blocks: Vec<Vec<&str>> = listed
        .split("\0\0")
        .filter(|block| !block.is_empty())
        .map(|block| block.split('\0').collect())
        .filter(|lines: &Vec<&str>| !lines.contains(&"bare"))
        .collect();
    // git leaves out a worktree whose record of its folder it can't read;
    // the facts name it by its git directory, as broken.
    let unrecorded = facts.common_dir.join("worktrees");
    let recorded = facts
        .worktrees
        .iter()
        .filter(|w| !w.path.starts_with(&unrecorded))
        .count();
    assert_eq!(blocks.len(), recorded, "{listed:?}");
    for lines in blocks {
        let field = |name: &str| {
            lines.iter().find_map(|line| {
                line.strip_prefix(name)
                    .and_then(|rest| rest.strip_prefix(' ').or((rest.is_empty()).then_some("")))
            })
        };
        let path = field("worktree").expect("a path");
        let path = if exists(path) {
            real(path)
        } else {
            path.to_owned()
        };
        let read = worktree(facts, &path);
        let head = field("HEAD").filter(|oid| oid.bytes().any(|b| b != b'0'));
        assert_eq!(
            read.head.as_ref().map(|h| h.oid.to_string()).as_deref(),
            head,
            "{path}: HEAD"
        );
        assert_eq!(
            read.branch.as_deref(),
            field("branch").map(|b| b.strip_prefix("refs/heads/").unwrap_or(b)),
            "{path}: branch"
        );
        assert_eq!(
            read.lock.as_ref().map(|l| l.lock.reason.as_str()),
            field("locked"),
            "{path}: lock"
        );
        if let Some(head) = &read.head {
            assert_eq!(head.merged, merged(s, repo, facts, &head.oid.to_string()));
        }
        if read.folder && read.broken.is_none() {
            let status: BTreeSet<PathBuf> = git_z(
                s,
                &path,
                &[
                    "--no-optional-locks",
                    "status",
                    "--porcelain=v1",
                    "-z",
                    "--no-renames",
                    "--untracked-files=all",
                ],
            )
            .iter()
            .map(|entry| PathBuf::from(entry[3..].trim_end_matches('/')))
            .collect();
            assert_eq!(
                read.uncommitted,
                status.into_iter().collect::<Vec<_>>(),
                "{path}: status"
            );
        }
    }
}

/// Whether git says `oid` is in the default branch; None without one.
fn merged(s: &Sandbox, repo: &str, facts: &RepositoryFacts, oid: &str) -> Option<bool> {
    facts.default.as_ref().map(|default| {
        let range = format!("{}..{oid}", default.reference);
        s.git(repo, &["rev-list", "--count", &range]).trim() == "0"
    })
}

fn assert_branches_agree(s: &Sandbox, repo: &str, facts: &RepositoryFacts) {
    let rows = s.git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(committerdate:unix)",
            "refs/heads",
        ],
    );
    let rows: Vec<Vec<&str>> = rows.lines().map(|row| row.split('\0').collect()).collect();
    assert_eq!(rows.len(), facts.branches.len(), "{rows:?}");
    for row in rows {
        let [name, oid, upstream, track, time] = row[..] else {
            panic!("{row:?}")
        };
        let name = name.strip_prefix("refs/heads/").expect("a branch");
        let read = branch(facts, name);
        assert_eq!(read.tip.oid.to_string(), oid, "{name}");
        assert_eq!(read.tip.time.as_second().to_string(), time, "{name}");
        assert_eq!(read.tip.merged, merged(s, repo, facts, oid), "{name}");
        let count = |word: &str| {
            track
                .split(", ")
                .find_map(|part| part.strip_prefix(word))
                .map_or(0, |n| n.trim().parse().expect("a count"))
        };
        assert_eq!(
            read.upstream
                .as_ref()
                .map(|u| (u.reference.as_str(), u.gone, u.ahead, u.behind)),
            (!upstream.is_empty()).then(|| (
                upstream,
                track == "gone",
                count("ahead "),
                count("behind ")
            )),
            "{name}: upstream"
        );
        assert_eq!(
            read.content_landed,
            facts
                .default
                .as_ref()
                .map(|d| content_landed(s, repo, &d.reference, oid)),
            "{name}: content landed"
        );
    }
}

/// Content landed as git computes it: every path `tip` changed since its
/// merge base has, in `default`'s tree, the mode and object `tip` gave it;
/// false with no merge base.
fn content_landed(s: &Sandbox, repo: &str, default: &str, tip: &str) -> bool {
    let count = |args: &[&str]| {
        let all: Vec<&str> = ["rev-list", "--count"]
            .into_iter()
            .chain(args.iter().copied())
            .collect();
        s.git(repo, &all).trim().to_owned()
    };
    let not_default = format!("^{default}");
    // Every commit of `tip` is missing from `default`: they share none.
    if count(&[tip, &not_default]) == count(&[tip]) {
        return false;
    }
    let base = s.git(repo, &["merge-base", default, tip]);
    let changes = git_z(
        s,
        repo,
        &[
            "diff-tree",
            "-r",
            "-z",
            "--no-renames",
            "--no-commit-id",
            base.trim(),
            tip,
        ],
    );
    changes.chunks(2).all(|change| {
        let [raw, path] = change else {
            panic!("{change:?}")
        };
        let fields: Vec<&str> = raw.trim_start_matches(':').split(' ').collect();
        let wanted = (fields[4] != "D").then(|| format!("{} {}", fields[1], fields[3]));
        let there = git_z(s, repo, &["ls-tree", "-z", default, "--", path])
            .first()
            .map(|entry| {
                let (meta, _) = entry.split_once('\t').expect("an entry");
                let parts: Vec<&str> = meta.split(' ').collect();
                format!("{} {}", parts[0], parts[2])
            });
        there == wanted
    })
}

// Sandboxes ---------------------------------------------------------------------------

/// A clone with a symbolic `origin/HEAD` and, in `<repo>.worktrees/`:
///
/// - `landed`: at `main`, pushed;
/// - `dirty`: a modified, a staged and an untracked file, and an ignored
///   build folder;
/// - `kept`: kept files and links equal to the main checkout's, others
///   differing, and some the main checkout lacks;
/// - `nested`: a nested repository;
/// - `detached`: a detached `HEAD`;
/// - `gone`: its folder deleted;
/// - `broken`: its `.git` naming nothing;
/// - `foreign`: its `.git` naming another repository;
/// - `swapped`: its `.git` naming `landed`'s git directory;
/// - `blank`: an empty record of its folder;
/// - `unrelated`: a branch with no history in common with `main`;
/// - `renamed`: a rename whose destination `main` holds, beside the
///   source;
/// - `locked-agent`, `locked-dead`, `locked-since`, `locked-other`,
///   `locked-empty`: a lock in each reason form;
/// - `empty`: a branch with no commit;
/// - `diverged`: ahead of and behind its upstream;
/// - `squashed`: its content on `main` in another commit;
///
/// and the branches `upstream-gone`, `tracks-main`, whose upstream is the
/// local `main`, and `no-remote`, whose remote isn't configured, with
/// most refs packed.
fn clone_with_everything(s: &Sandbox) -> String {
    let repo = s.cloned();
    let wt = |name: &str| join(&format!("{repo}.worktrees"), name);
    s.commit(&repo, "src/lib.rs", "one\n");
    s.commit(&repo, "README.txt", "read me\n");
    s.git(&repo, &["push", "--quiet", "origin", "main"]);

    let landed = s.add_worktree(&repo, "landed");
    s.git(
        &landed,
        &["push", "--quiet", "--set-upstream", "origin", "landed"],
    );

    let dirty = s.add_worktree(&repo, "dirty");
    s.write(&join(&dirty, "src/lib.rs"), "two\n");
    s.write(&join(&dirty, "staged.txt"), "staged\n");
    s.git(&dirty, &["add", "staged.txt"]);
    s.write(&join(&dirty, "notes/todo.txt"), "todo\n");
    s.write(&join(&dirty, "build/out.o"), "object\n");

    let kept = s.add_worktree(&repo, "kept");
    for (rel, main, mine) in [
        (".env", Some("A=1\n"), "A=1\n"),
        (".env.local", Some("B=1\n"), "B=2\n"),
        (".envrc", None, "use flake\n"),
    ] {
        main.into_iter()
            .for_each(|text| s.write(&join(&repo, rel), text));
        s.write(&join(&kept, rel), mine);
    }
    let symlink = |target: &str, at: &str| {
        std::os::unix::fs::symlink(target, at).unwrap_or_else(|e| panic!("{at}: {e}"))
    };
    symlink("/secrets/env", &join(&kept, ".env.same"));
    symlink("/secrets/env", &join(&repo, ".env.same"));
    symlink("/secrets/a", &join(&kept, ".env.other"));
    symlink("/secrets/b", &join(&repo, ".env.other"));
    symlink("/secrets/env", &join(&kept, ".env.file"));
    s.write(&join(&repo, ".env.file"), "A=1\n");

    let nested = s.add_worktree(&repo, "nested");
    s.sh(
        &["git", "init", "--quiet", &join(&nested, "nested/lib")],
        None,
    );

    s.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            &wt("detached"),
            "HEAD~1",
        ],
    );

    let gone = s.add_worktree(&repo, "gone");
    fs::remove_dir_all(&gone).expect("the folder goes");

    let broken = s.add_worktree(&repo, "broken");
    s.write(&join(&broken, ".git"), "gitdir: /nowhere\n");

    let other = s.repo_named("other", &[]);
    let foreign = s.add_worktree(&repo, "foreign");
    s.write(
        &join(&foreign, ".git"),
        &format!("gitdir: {}\n", join(&other, ".git")),
    );
    let swapped = s.add_worktree(&repo, "swapped");
    s.write(
        &join(&swapped, ".git"),
        &format!("gitdir: {}\n", join(&repo, ".git/worktrees/landed")),
    );

    let agent = format!(
        "claude agent abc (pid {} start Thu Nov 19 09:00:00 2026)",
        std::process::id()
    );
    let dead = format!("claude agent def (pid {NO_SUCH_PID} start Thu Nov 19 09:00:00 2026)");
    for (name, reason) in [
        ("locked-agent", Some(agent.as_str())),
        ("locked-dead", Some(dead.as_str())),
        ("locked-since", Some("neil since 2026-11-19T09:00:00Z")),
        ("locked-other", Some("on a USB disk")),
        ("locked-empty", None),
    ] {
        let path = s.add_worktree(&repo, name);
        lock(s, &repo, &path, reason);
    }

    s.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--orphan",
            "-b",
            "empty",
            &wt("empty"),
        ],
    );

    let diverged = s.add_worktree(&repo, "diverged");
    s.commit(&diverged, "pushed.txt", "pushed\n");
    s.git(
        &diverged,
        &["push", "--quiet", "--set-upstream", "origin", "diverged"],
    );
    s.git(&diverged, &["reset", "--quiet", "--hard", "HEAD~1"]);
    s.commit(&diverged, "local.txt", "local\n");

    s.git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "--orphan",
            "-b",
            "unrelated",
            &wt("unrelated"),
        ],
    );
    s.commit(&wt("unrelated"), "other.txt", "other\n");

    let renamed = s.add_worktree(&repo, "renamed");
    s.git(&renamed, &["mv", "README.txt", "NOTES.txt"]);
    s.git(&renamed, &["commit", "--quiet", "-m", "rename"]);
    s.write(&join(&repo, "NOTES.txt"), "read me\n");
    s.git(&repo, &["add", "NOTES.txt"]);

    let squashed = s.add_worktree(&repo, "squashed");
    s.commit(&squashed, "feature/a.txt", "a\n");
    s.commit(&squashed, "feature/b.txt", "b\n");
    s.git(&squashed, &["rm", "--quiet", "src/lib.rs"]);
    s.git(&squashed, &["commit", "--quiet", "-m", "remove src/lib.rs"]);
    s.write(&join(&repo, "feature/a.txt"), "a\n");
    s.write(&join(&repo, "feature/b.txt"), "b\n");
    s.git(&repo, &["add", "feature"]);
    s.git(&repo, &["rm", "--quiet", "src/lib.rs"]);
    s.git(&repo, &["commit", "--quiet", "-m", "squash"]);
    s.git(&repo, &["push", "--quiet", "origin", "main"]);

    s.git(&repo, &["branch", "upstream-gone", "main"]);
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
    s.git(
        &join(&s.tmp, "repo.remote.git"),
        &["branch", "--quiet", "-D", "upstream-gone"],
    );
    s.git(&repo, &["fetch", "--quiet", "--prune", "origin"]);

    s.git(
        &repo,
        &["branch", "--quiet", "--track", "tracks-main", "main"],
    );
    s.git(&repo, &["branch", "no-remote", "main"]);
    for (key, value) in [("remote", "nowhere"), ("merge", "refs/heads/no-remote")] {
        s.git(
            &repo,
            &["config", &format!("branch.no-remote.{key}"), value],
        );
    }

    s.git(&repo, &["pack-refs", "--all"]);
    s.git(&repo, &["branch", "loose", "main"]);

    // Last, since `git worktree add` prunes an entry like this.
    s.add_worktree(&repo, "blank");
    s.write(&join(&repo, ".git/worktrees/blank/gitdir"), "");
    repo
}

// The clone ---------------------------------------------------------------------------

#[test]
fn test_a_clone_agrees_with_git() {
    let s = Sandbox::new();
    let repo = clone_with_everything(&s);
    let read = facts(&s, &repo, &[]).facts;
    assert_agrees_with_git(&s, &repo, &read);

    let wt = |name: &str| real(&join(&format!("{repo}.worktrees"), name));
    assert_eq!(read.path, PathBuf::from(&repo));
    assert_eq!(read.common_dir, PathBuf::from(join(&repo, ".git")));
    assert!(!read.bare);
    let default = read.default.as_ref().expect("a default branch");
    assert_eq!(
        (default.branch.as_str(), default.reference.as_str()),
        ("main", "refs/remotes/origin/main")
    );
    assert_eq!(default.source, DefaultSource::OriginHead);

    let main = &read.worktrees[0];
    assert_eq!((main.path.as_path(), main.main), (Path::new(&repo), true));
    assert!(read.worktrees[1..].iter().all(|w| !w.main));
    assert!(read.worktrees.iter().all(|w| w.changed.is_some()));
    assert!(read.worktrees.iter().all(|w| w.bytes.is_none()));

    let dirty = worktree(&read, &wt("dirty"));
    assert_eq!(
        dirty.uncommitted,
        paths(&["notes/todo.txt", "src/lib.rs", "staged.txt"])
    );
    assert_eq!(
        worktree(&read, &wt("kept")).own_files,
        paths(&[".env.file", ".env.local", ".env.other", ".envrc"])
    );
    assert!(worktree(&read, &wt("kept")).uncommitted.is_empty());
    assert_eq!(
        worktree(&read, &wt("nested")).own_files,
        paths(&["nested/lib"])
    );
    assert!(worktree(&read, &wt("landed")).own_files.is_empty());

    let detached = worktree(&read, &wt("detached"));
    assert_eq!(detached.branch, None);
    assert_eq!(detached.head.as_ref().and_then(|h| h.merged), Some(true));

    let gone = worktree(&read, &join(&format!("{repo}.worktrees"), "gone"));
    assert_eq!((gone.folder, gone.broken.as_deref()), (false, None));
    assert_eq!(gone.branch.as_deref(), Some("gone"));

    let broken = worktree(&read, &wt("broken"));
    assert!(broken.folder);
    assert!(broken.broken.is_some(), "{broken:?}");
    assert_eq!(broken.branch.as_deref(), Some("broken"));
    for name in ["foreign", "swapped"] {
        let found = worktree(&read, &wt(name));
        assert!(found.folder && found.broken.is_some(), "{found:?}");
    }
    let blank = worktree(&read, &join(&repo, ".git/worktrees/blank"));
    assert!(!blank.folder && blank.broken.is_some(), "{blank:?}");
    assert_eq!(blank.branch.as_deref(), Some("blank"));

    let empty = worktree(&read, &wt("empty"));
    assert_eq!(
        (empty.branch.as_deref(), &empty.head),
        (Some("empty"), &None)
    );
    assert!(read.branches.iter().all(|b| b.name != "empty"));

    let lock = |name: &str| {
        worktree(&read, &wt(name))
            .lock
            .clone()
            .unwrap_or_else(|| panic!("{name} is locked"))
    };
    let agent = lock("locked-agent");
    assert_eq!(
        (agent.lock.form, agent.lock.pid),
        (LockForm::ClaudeAgent, Some(std::process::id()))
    );
    assert!(agent.process_start.is_some(), "this test's process runs");
    assert!(agent.written.is_some());
    let dead = lock("locked-dead");
    assert_eq!(
        (dead.lock.pid, dead.process_start),
        (Some(NO_SUCH_PID), None)
    );
    let since = lock("locked-since");
    assert_eq!(
        (since.lock.form, since.lock.who.as_str(), since.lock.since),
        (
            LockForm::Since,
            "neil",
            Some("2026-11-19T09:00:00Z".parse().expect("a time"))
        )
    );
    assert_eq!(lock("locked-other").lock.form, LockForm::Other);
    assert_eq!(lock("locked-empty").lock.reason, "");
    assert!(worktree(&read, &wt("landed")).lock.is_none());

    let squashed = branch(&read, "squashed");
    assert_eq!(
        (squashed.tip.merged, squashed.content_landed),
        (Some(false), Some(true))
    );
    assert_eq!(branch(&read, "diverged").content_landed, Some(false));
    // Read as a rename, the change would be only at its destination, which
    // `main` holds; read path by path, `main` still holds the source.
    assert_eq!(branch(&read, "renamed").content_landed, Some(false));
    let unrelated = branch(&read, "unrelated");
    assert_eq!(
        (unrelated.tip.merged, unrelated.content_landed),
        (Some(false), Some(false))
    );
    let diverged = branch(&read, "diverged")
        .upstream
        .clone()
        .expect("an upstream");
    assert_eq!(
        (diverged.gone, diverged.ahead, diverged.behind),
        (false, 1, 1)
    );
    let gone_upstream = branch(&read, "upstream-gone")
        .upstream
        .clone()
        .expect("an upstream");
    assert_eq!(
        (gone_upstream.reference.as_str(), gone_upstream.gone),
        ("refs/remotes/origin/upstream-gone", true)
    );
    assert_eq!(branch(&read, "dirty").upstream, None);
    assert_eq!(branch(&read, "no-remote").upstream, None);
    assert_eq!(
        branch(&read, "tracks-main")
            .upstream
            .as_ref()
            .map(|u| u.reference.as_str()),
        Some("refs/heads/main")
    );
    assert_eq!(branch(&read, "landed").tip.merged, Some(true));
    assert_eq!(branch(&read, "loose").tip.merged, Some(true));
}

#[test]
fn test_any_worktree_reads_the_same_repository() {
    let s = Sandbox::new();
    let repo = clone_with_everything(&s);
    let from_main = facts(&s, &repo, &[]).facts;
    for name in ["dirty", "detached", "locked-agent"] {
        let path = join(&format!("{repo}.worktrees"), name);
        let read = facts(&s, &path, &[]).facts;
        assert_eq!(read.common_dir, from_main.common_dir, "{name}");
        assert_eq!(read.worktrees.len(), from_main.worktrees.len(), "{name}");
    }
}

#[test]
fn test_bytes_only_with_size() {
    let s = Sandbox::new();
    let repo = clone_with_everything(&s);
    let read = facts(&s, &repo, &["--size"]).facts;
    for worktree in &read.worktrees {
        assert_eq!(
            worktree.bytes.is_some_and(|bytes| bytes > 0),
            worktree.folder,
            "{worktree:?}"
        );
    }
}

#[test]
fn test_the_backups_archives() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let origin = join(&s.tmp, "repo.remote.git");
    let place = backup::location(&s.environment(), Some(&origin), "repo").expect("a HOME");
    let place = place.to_string_lossy().into_owned();
    assert!(facts(&s, &repo, &[]).facts.backup.archives.is_empty());
    s.bare(&place);
    let tip = s.git(&repo, &["rev-parse", "HEAD"]).trim().to_owned();
    s.git(
        &repo,
        &[
            "push",
            "--quiet",
            "--no-verify",
            &place,
            &format!("{tip}:refs/archive/main/2026-11-19T091200Z"),
        ],
    );
    let read = facts(&s, &repo, &[]).facts;
    assert_eq!(read.backup.path, PathBuf::from(&place));
    let archives: Vec<(&str, String)> = read
        .backup
        .archives
        .iter()
        .map(|a| (a.branch.as_str(), a.oid.to_string()))
        .collect();
    assert_eq!(archives, vec![("main", tip)]);
    assert_eq!(
        read.backup.archives[0].compared,
        Some(true),
        "the archive of a branch's tip is checked"
    );
}

// Other shapes of repository ----------------------------------------------------------

#[test]
fn test_a_bare_repository_has_no_main_worktree() {
    let s = Sandbox::new();
    s.cloned();
    let bare = join(&s.tmp, "app.git");
    let origin = join(&s.tmp, "repo.remote.git");
    s.sh(&["git", "clone", "--quiet", "--bare", &origin, &bare], None);
    s.git(
        &bare,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/remotes/origin/*",
        ],
    );
    s.git(&bare, &["fetch", "--quiet", "origin"]);
    s.git(&bare, &["remote", "set-head", "origin", "main"]);
    let one = join(&s.tmp, "app.worktrees/one");
    s.git(
        &bare,
        &["worktree", "add", "--quiet", "-b", "one", &one, "main"],
    );
    s.git(
        &bare,
        &[
            "worktree",
            "add",
            "--quiet",
            &join(&s.tmp, "app.worktrees/main"),
            "main",
        ],
    );
    s.write(&join(&one, ".env"), "A=1\n");

    for checkout in [bare.as_str(), one.as_str()] {
        let read = facts(&s, checkout, &[]).facts;
        assert_agrees_with_git(&s, &bare, &read);
        assert!(read.bare);
        assert_eq!(read.path, PathBuf::from(real(&bare)));
        assert_eq!(read.common_dir, PathBuf::from(real(&bare)));
        assert_eq!(read.worktrees.len(), 2);
        assert!(read.worktrees.iter().all(|w| !w.main));
        // With no main checkout to compare with, every kept file is its own.
        assert_eq!(worktree(&read, &real(&one)).own_files, paths(&[".env"]));
        assert_eq!(
            read.default.as_ref().map(|d| d.source),
            Some(DefaultSource::OriginHead)
        );
    }
}

/// A symbolic ref under `refs/heads/` is no branch of its own: deleting
/// it as one would delete the branch it names.
#[test]
fn test_a_symbolic_branch_is_left_out() {
    let s = Sandbox::new();
    let repo = s.cloned();
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/master", "refs/heads/main"],
    );
    s.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/heads/dangling",
            "refs/heads/never-made",
        ],
    );
    let read = facts(&s, &repo, &[]).facts;
    let names: Vec<&str> = read.branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["main"]);
}

/// A worktree whose `HEAD` names an alias is on the branch the alias
/// names, at the end of the chain.
#[test]
fn test_a_worktree_on_an_alias_is_on_the_branch_it_names() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let path = s.add_worktree(&repo, "feature");
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/middle", "refs/heads/feature"],
    );
    s.git(
        &repo,
        &["symbolic-ref", "refs/heads/alias", "refs/heads/middle"],
    );
    s.git(&path, &["symbolic-ref", "HEAD", "refs/heads/alias"]);
    let read = facts(&s, &repo, &[]).facts;
    assert_eq!(worktree(&read, &path).branch.as_deref(), Some("feature"));
}

#[test]
fn test_no_origin_means_no_default_branch() {
    let s = Sandbox::new();
    let repo = s.repo(&[("a.txt", "a\n")]);
    let feat = s.add_worktree(&repo, "feat");
    s.commit(&feat, "b.txt", "b\n");
    let read = facts(&s, &repo, &["--default-branch", "main"]).facts;
    assert_agrees_with_git(&s, &repo, &read);
    assert_eq!(read.default, None);
    assert!(read.remote_refs.is_empty());
    assert!(
        read.branches
            .iter()
            .all(|b| b.tip.merged.is_none() && b.content_landed.is_none() && b.upstream.is_none())
    );
    assert_eq!(read.worktrees[1].head.as_ref().and_then(|h| h.merged), None);
    assert_eq!(read.backup.path.file_name(), Some("repo.git".as_ref()));
}

#[test]
fn test_without_origin_head_the_named_default_branch() {
    let s = Sandbox::new();
    let repo = s.cloned();
    s.git(&repo, &["remote", "set-head", "origin", "--delete"]);
    assert_eq!(facts(&s, &repo, &[]).facts.default, None);
    let read = facts(&s, &repo, &["--default-branch", "main"]).facts;
    let default = read.default.expect("a default branch");
    assert_eq!(
        (default.branch.as_str(), default.source),
        ("main", DefaultSource::Option)
    );
    for missing in ["trunk", "not a ref"] {
        assert_eq!(
            facts(&s, &repo, &["--default-branch", missing])
                .facts
                .default,
            None,
            "{missing}"
        );
    }
}

/// The default branch `read::repository` finds in `repo` given the names
/// `named`, first first.
fn default_given(
    s: &Sandbox,
    repo: &str,
    named: &[(DefaultSource, &str)],
) -> Option<(String, DefaultSource)> {
    let target = Target {
        path: PathBuf::from(repo),
        default_branches: named
            .iter()
            .map(|(source, branch)| (*source, (*branch).to_owned()))
            .collect(),
    };
    read::repository(&Real::new(s.environment()), &target, false)
        .expect("the facts")
        .default
        .map(|d| (d.branch, d.source))
}

#[test]
fn test_the_default_branchs_sources_in_order() {
    let s = Sandbox::new();
    let repo = s.cloned();
    s.git(&repo, &["push", "--quiet", "origin", "main:other"]);
    s.git(&repo, &["fetch", "--quiet", "origin"]);
    let manifest = (DefaultSource::Manifest, "other");
    let option = (DefaultSource::Option, "other");
    assert_eq!(
        default_given(&s, &repo, &[manifest, option]),
        Some(("main".to_owned(), DefaultSource::OriginHead)),
        "origin/HEAD comes first"
    );
    s.git(
        &repo,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/gone",
        ],
    );
    assert_eq!(
        default_given(&s, &repo, &[(DefaultSource::Manifest, "missing"), option]),
        Some(("other".to_owned(), DefaultSource::Option)),
        "a dangling origin/HEAD and a missing name fall through"
    );
    assert_eq!(
        default_given(&s, &repo, &[manifest, (DefaultSource::Option, "main")]),
        Some(("other".to_owned(), DefaultSource::Manifest)),
        "the manifest's name comes before the option's"
    );
}

#[test]
fn test_a_lock_stays_when_ps_cannot_run() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    let held = s.add_worktree(&repo, "held");
    let reason = format!(
        "claude agent abc (pid {} start Thu Nov 19 09:00:00 2026)",
        std::process::id()
    );
    lock(&s, &repo, &held, Some(&reason));
    fs::remove_dir_all(&held).expect("the folder goes");
    let ran = s.run(
        &["facts", "--repository", &repo, "--format", "json"],
        None,
        &[("PATH", "/nonexistent")],
    );
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let read: Printed = serde_json::from_str(&ran.stdout).expect("the facts");
    let held = worktree(&read.facts, &held);
    assert!(!held.folder);
    assert_eq!(held.broken, None);
    let lock = held.lock.as_ref().expect("the lock stays");
    assert_eq!(
        (lock.lock.pid, lock.process_start),
        (Some(std::process::id()), None)
    );
}

#[test]
fn test_settings_from_the_repositorys_config() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    assert_eq!(facts(&s, &repo, &[]).settings, Settings::default());
    for (key, value) in [
        ("worktreeManager.unattended", "sweep"),
        ("worktreeManager.idle", "2h"),
        ("worktreeManager.disposable", "tmp/*"),
        ("worktreeManager.disposable", "scratch-*"),
    ] {
        s.git(&repo, &["config", "--add", key, value]);
    }
    let settings = facts(&s, &repo, &[]).settings;
    assert_eq!(settings.unattended, Unattended::Sweep);
    assert_eq!(settings.idle, SignedDuration::from_hours(2));
    assert_eq!(settings.disposable, ["tmp/*", "scratch-*"]);
}

// The command -------------------------------------------------------------------------

#[test]
fn test_outside_a_repository_facts_fails_and_prints_nothing() {
    let s = Sandbox::new();
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    for args in [&["facts"][..], &["facts", "--format", "json"]] {
        let ran = s.run(args, Some(&plain), &[]);
        assert_eq!((ran.code, ran.stdout.as_str()), (1, ""), "{args:?}");
        assert!(
            ran.stderr.contains("not in a git repository"),
            "{}",
            ran.stderr
        );
    }
}

#[test]
fn test_every_format_prints_the_json() {
    let s = Sandbox::new();
    let repo = s.cloned();
    let text = s.run(&["facts"], Some(&repo), &[]);
    assert_eq!(text.code, 0, "{}", text.stderr);
    let json = s.run(&["facts", "--format", "json"], Some(&repo), &[]);
    assert_eq!(text.stdout, json.stdout);
    let value: serde_json::Value = serde_json::from_str(&json.stdout).expect("JSON");
    assert_eq!(value["command"], "facts");
}

// Reading changes nothing ---------------------------------------------------------------

/// Every ref, the worktree list, and every file's and folder's
/// modification time and size under the sandbox, the state folder and the
/// backup among them.
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
fn test_reading_changes_nothing() {
    let s = Sandbox::new();
    let repo = clone_with_everything(&s);
    let state = join(&s.tmp, "state");
    fs::create_dir_all(&state).expect("the state folder");
    let before = snapshot(&s, &repo);

    let system = Real::new(s.environment());
    let target = Target {
        path: PathBuf::from(&repo),
        default_branches: vec![],
    };
    let read = read::repository(&system, &target, true).expect("the facts");
    read::settings(&read.common_dir).expect("the settings");
    assert_eq!(snapshot(&s, &repo), before, "read::repository wrote");

    for args in [&["facts", "--size"][..], &["facts", "--format", "json"]] {
        let ran = s.run(args, Some(&repo), &[]);
        assert_eq!(ran.code, 0, "{}", ran.stderr);
        assert_eq!(snapshot(&s, &repo), before, "{args:?} wrote");
    }
}
