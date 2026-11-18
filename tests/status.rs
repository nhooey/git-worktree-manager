//! `status`, run as the binary over sandbox repositories: the plan of each
//! worktree and branch, the repositories a manifest names, a named
//! target, and the proof that it writes nothing.

mod sandbox;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use git_worktree_manager::model::ids::CommandName;
use git_worktree_manager::model::report::Report;
use sandbox::{Sandbox, join, real, utime};
use serde_json::Value;

const FORMATS: [&str; 3] = ["text", "json", "markdown"];

/// A clone whose worktrees each meet a different rule, idle at once:
/// - `landed`: on `main`'s commit, removed, its branch deleted;
/// - `feat`: a commit of its own, kept;
/// - `gone`: a detached `HEAD` on a commit of its own, its folder
///   deleted, pruned once that commit is archived;
/// - `held`: landed, and locked.
struct Clone {
    repo: String,
    landed: String,
    feat: String,
    gone: String,
    held: String,
}

fn clone(s: &Sandbox) -> Clone {
    let repo = s.cloned();
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let landed = s.add_worktree(&repo, "landed");
    let feat = s.add_worktree(&repo, "feat");
    s.commit(&feat, "feat.txt", "feat\n");
    let gone = join(&format!("{repo}.worktrees"), "gone");
    s.git(
        &repo,
        &["worktree", "add", "--quiet", "--detach", &gone, "HEAD"],
    );
    let gone = real(&gone);
    s.commit(&gone, "lost.txt", "lost\n");
    fs::remove_dir_all(&gone).expect("the folder goes");
    let held = s.add_worktree(&repo, "held");
    s.git(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "neil since 2026-11-19T09:00:00Z",
            &held,
        ],
    );
    Clone {
        repo,
        landed,
        feat,
        gone,
        held,
    }
}

/// `status <args> --format json`, expecting `code`.
fn status(s: &Sandbox, args: &[&str], code: i32) -> Value {
    let args: Vec<&str> = ["status"].into_iter().chain(args.iter().copied()).collect();
    s.run_json(&args, None, code)
}

/// The one item of `list` whose `key` is `value`.
fn find<'a>(list: &'a Value, key: &str, value: &str) -> &'a Value {
    list.as_array()
        .and_then(|items| items.iter().find(|item| item[key] == value))
        .unwrap_or_else(|| panic!("no {key} {value} in {list:#}"))
}

fn worktree<'a>(repository: &'a Value, path: &str) -> &'a Value {
    find(&repository["worktrees"], "path", path)
}

fn branch<'a>(repository: &'a Value, name: &str) -> &'a Value {
    find(&repository["branches"], "name", name)
}

fn codes(notes: &Value) -> Vec<&str> {
    notes
        .as_array()
        .map(|notes| notes.iter().filter_map(|n| n["code"].as_str()).collect())
        .unwrap_or_default()
}

// The plan ------------------------------------------------------------------------

#[test]
fn test_a_landed_worktree_is_removed_and_its_branch_deleted() {
    let s = Sandbox::new();
    let c = clone(&s);
    let report = status(&s, &["--repository", &c.repo], 0);
    let repository = &report["repositories"][0];
    let landed = worktree(repository, &c.landed);
    assert_eq!(
        (&landed["action"], &landed["rule"], &landed["outcome"]),
        (
            &Value::from("remove"),
            &Value::from("worktree.remove"),
            &Value::from("planned")
        )
    );
    assert_eq!(landed["main"], false);
    let branch = branch(repository, "landed");
    assert_eq!(
        (&branch["action"], &branch["rule"], &branch["outcome"]),
        (
            &Value::from("delete"),
            &Value::from("branch.landed"),
            &Value::from("planned")
        )
    );
    assert_eq!(branch["archive"], Value::Null, "a merged tip needs none");
    assert_eq!(worktree(repository, &c.repo)["main"], true);
}

#[test]
fn test_an_unmerged_worktree_is_kept_with_its_reasons() {
    let s = Sandbox::new();
    let c = clone(&s);
    let report = status(&s, &["--repository", &c.repo], 0);
    let repository = &report["repositories"][0];
    let feat = worktree(repository, &c.feat);
    assert_eq!(
        (&feat["action"], &feat["outcome"]),
        (&Value::from("keep"), &Value::from("none"))
    );
    assert_eq!(codes(&feat["reasons"]), ["not-landed"]);
    let branch = branch(repository, "feat");
    assert_eq!(
        (&branch["action"], &branch["state"]),
        (&Value::from("keep"), &Value::from("local-only"))
    );
}

#[test]
fn test_a_missing_folder_is_pruned_after_its_detached_head_is_archived() {
    let s = Sandbox::new();
    let c = clone(&s);
    let report = status(&s, &["--repository", &c.repo], 0);
    let repository = &report["repositories"][0];
    let gone = worktree(repository, &c.gone);
    assert_eq!(
        (&gone["action"], &gone["rule"], &gone["outcome"]),
        (
            &Value::from("prune"),
            &Value::from("worktree.prune"),
            &Value::from("planned")
        )
    );
    assert_eq!(gone["branch"], Value::Null);
    assert_eq!(gone["archive"]["state"], "planned");
    let reference = gone["archive"]["ref"].as_str().expect("an archive ref");
    assert!(
        reference.starts_with("refs/archive/detached-head/gone/"),
        "{reference}"
    );
    assert_eq!(repository["totals"]["archives.planned"], 1);
    for format in ["text", "markdown"] {
        let ran = s.run(
            &["status", "--repository", &c.repo, "--format", format],
            None,
            &[],
        );
        assert_eq!(ran.code, 0, "{}", ran.stderr);
        assert!(ran.stdout.contains(reference), "{format}: {}", ran.stdout);
    }
}

#[test]
fn test_a_pruned_worktree_shares_its_deleted_branchs_archive() {
    let s = Sandbox::new();
    let c = clone(&s);
    let preview = s.add_worktree(&c.repo, "preview/try");
    s.commit(&preview, "try.txt", "try\n");
    fs::remove_dir_all(&preview).expect("the folder goes");
    let report = status(&s, &["--repository", &c.repo, "preview/try"], 0);
    let repository = &report["repositories"][0];
    let pruned = worktree(repository, &preview);
    let deleted = branch(repository, "preview/try");
    assert_eq!(
        (&pruned["action"], &deleted["action"], &deleted["rule"]),
        (
            &Value::from("prune"),
            &Value::from("delete"),
            &Value::from("branch.disposable")
        )
    );
    assert_eq!(pruned["archive"], deleted["archive"]);
    assert_eq!(deleted["archive"]["state"], "planned");
    assert_eq!(repository["totals"]["archives.planned"], 1);
}

#[test]
fn test_a_locked_worktree_is_kept() {
    let s = Sandbox::new();
    let c = clone(&s);
    let report = status(&s, &["--repository", &c.repo], 0);
    let repository = &report["repositories"][0];
    let held = worktree(repository, &c.held);
    assert_eq!(held["action"], "keep");
    assert_eq!(codes(&held["reasons"]), ["locked"]);
    assert_eq!(held["lock"]["who"], "neil");
    let branch = branch(repository, "held");
    assert_eq!(branch["action"], "keep");
    assert_eq!(codes(&branch["reasons"]), ["checked-out"]);
}

/// A bare clone of the sandbox's remote, `app.git`, with the linked
/// worktrees `one` and `two`; the sandbox's clone must exist.
fn bare_with_worktrees(s: &Sandbox) -> String {
    let bare = join(&s.tmp, "app.git");
    let origin = join(&s.tmp, "repo.remote.git");
    s.sh(&["git", "clone", "--quiet", "--bare", &origin, &bare], None);
    let one = join(&s.tmp, "app.worktrees/one");
    let two = join(&s.tmp, "app.worktrees/two");
    s.git(
        &bare,
        &["worktree", "add", "--quiet", "-b", "one", &one, "main"],
    );
    s.git(
        &bare,
        &["worktree", "add", "--quiet", "-b", "two", &two, "main"],
    );
    real(&bare)
}

#[test]
fn test_a_bare_repository_marks_no_worktree_main() {
    let s = Sandbox::new();
    s.cloned();
    let bare = bare_with_worktrees(&s);
    let one = join(&s.tmp, "app.worktrees/one");
    for checkout in [&bare, &one] {
        let report = status(&s, &["--repository", checkout], 0);
        let worktrees = report["repositories"][0]["worktrees"]
            .as_array()
            .expect("worktrees")
            .clone();
        assert_eq!(worktrees.len(), 2, "{checkout}");
        assert!(worktrees.iter().all(|w| w["main"] == false), "{checkout}");
    }
}

#[test]
fn test_with_no_default_branch_only_a_gone_folder_is_pruned() {
    let s = Sandbox::new();
    let repo = s.repo(&[]);
    s.git(&repo, &["config", "worktreeManager.idle", "0s"]);
    let landed = s.add_worktree(&repo, "landed");
    let gone = s.add_worktree(&repo, "gone");
    fs::remove_dir_all(&gone).expect("the folder goes");
    let report = status(&s, &["--repository", &repo], 0);
    let repository = &report["repositories"][0];
    assert_eq!(repository["problem"]["code"], "no-default-branch");
    assert_eq!(repository["default"], Value::Null);
    assert_eq!(worktree(repository, &landed)["action"], "keep");
    assert_eq!(worktree(repository, &gone)["action"], "prune");
    let branches = repository["branches"].as_array().expect("branches");
    assert!(branches.iter().all(|b| b["action"] == "keep"));
}

#[test]
fn test_the_default_branch_option_is_the_last_fallback() {
    let s = Sandbox::new();
    let c = clone(&s);
    s.git(&c.repo, &["remote", "set-head", "origin", "--delete"]);
    let without = status(&s, &["--repository", &c.repo], 0);
    assert_eq!(
        without["repositories"][0]["problem"]["code"],
        "no-default-branch"
    );
    let with = status(
        &s,
        &["--repository", &c.repo, "--default-branch", "main"],
        0,
    );
    assert_eq!(with["repositories"][0]["default"]["branch"], "main");
    assert_eq!(
        worktree(&with["repositories"][0], &c.landed)["action"],
        "remove"
    );
}

#[test]
fn test_size_measures_bytes() {
    let s = Sandbox::new();
    let c = clone(&s);
    let without = status(&s, &["--repository", &c.repo], 0);
    assert_eq!(
        worktree(&without["repositories"][0], &c.feat)["bytes"],
        Value::Null
    );
    let with = status(&s, &["--repository", &c.repo, "--size"], 0);
    assert!(worktree(&with["repositories"][0], &c.feat)["bytes"].is_u64());
}

#[test]
fn test_outside_a_repository_prints_nothing_and_exits_1() {
    let s = Sandbox::new();
    let plain = join(&s.tmp, "plain");
    fs::create_dir_all(&plain).expect("a folder");
    for format in FORMATS {
        for args in [
            vec!["status", "--format", format],
            vec!["status", "--repository", &plain, "--format", format],
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

// The JSON --------------------------------------------------------------------------

#[test]
fn test_the_json_is_the_models_report() {
    let s = Sandbox::new();
    let c = clone(&s);
    let ran = s.run(&["status", "--format", "json"], Some(&c.feat), &[]);
    assert_eq!(ran.code, 0, "{}", ran.stderr);
    let report: Report = serde_json::from_str(&ran.stdout).expect("the model's report");
    assert_eq!(report.schema, 1);
    assert_eq!(report.command, CommandName::Status);
    assert!(report.dry_run);
    assert_eq!(report.repositories.len(), 1);
    assert_eq!(report.repositories[0].path, PathBuf::from(&c.repo));
    assert_eq!(report.totals["repositories"], 1);
}

#[test]
fn test_every_rule_and_code_status_names_is_in_rules() {
    let s = Sandbox::new();
    let c = clone(&s);
    s.write(&join(&c.feat, "notes.txt"), "uncommitted\n");
    let preview = s.add_worktree(&c.repo, "preview/try");
    s.commit(&preview, "try.txt", "try\n");
    let bare = bare_with_worktrees(&s);
    let plain = s.repo_named("plain", &[]);
    s.add_worktree(&plain, "side");
    let manifest = join(&s.tmp, "repos.json");
    s.write(
        &manifest,
        &serde_json::json!([{"path": c.repo}, {"path": bare}, {"path": plain}]).to_string(),
    );
    let report = status(&s, &["--repositories", &manifest], 0);
    let rules = s.run_json(&["rules"], None, 0);
    let listed = |list: &str, key: &str| -> BTreeSet<String> {
        rules[list]
            .as_array()
            .unwrap_or_else(|| panic!("no {list} in rules"))
            .iter()
            .filter_map(|entry| entry[key].as_str().map(str::to_owned))
            .collect()
    };
    let rule_ids: BTreeSet<String> = listed("branch_rules", "id")
        .into_iter()
        .chain(listed("worktree_rules", "id"))
        .collect();
    let repositories = report["repositories"].as_array().expect("repositories");
    assert_eq!(repositories.len(), 3);
    let items: Vec<&Value> = repositories
        .iter()
        .flat_map(|repository| {
            ["worktrees", "branches"]
                .iter()
                .flat_map(|list| repository[*list].as_array().expect("items"))
        })
        .collect();
    let named: BTreeSet<String> = items
        .iter()
        .filter_map(|item| item["rule"].as_str().map(str::to_owned))
        .collect();
    assert!(named.contains("branch.disposable"), "{named:?}");
    assert!(named.contains("branch.no-default"), "{named:?}");
    assert!(named.is_subset(&rule_ids), "{named:?} not in {rule_ids:?}");
    let noted = |key: &str| -> BTreeSet<String> {
        items
            .iter()
            .flat_map(|item| codes(&item[key]))
            .map(str::to_owned)
            .collect()
    };
    let (reasons, attention) = (noted("reasons"), noted("attention"));
    assert!(reasons.is_subset(&listed("reasons", "code")), "{reasons:?}");
    assert!(attention.contains("uncommitted"), "{attention:?}");
    assert!(
        attention.is_subset(&listed("attention", "code")),
        "{attention:?}"
    );
}

// The repositories a manifest names ------------------------------------------------------

#[test]
fn test_a_manifest_skips_a_missing_clone_and_reports_an_unreadable_one() {
    let s = Sandbox::new();
    let c = clone(&s);
    let other = s.repo_named("other", &[("a.txt", "a\n")]);
    let broken = unreadable(&s, "broken");
    let src = join(&s.tmp, "src");
    let manifest = join(&s.tmp, "repos.json");
    s.write(
        &manifest,
        r#"[{"path": "../repo"}, {"path": "../other", "default_branch": "main"},
            {"path": "missing"}, {"path": "../broken"}]"#,
    );
    let home = s.var("HOME");
    for format in FORMATS {
        let ran = s.run(
            &[
                "status",
                "--repositories",
                &manifest,
                "--root",
                &src,
                "--format",
                format,
            ],
            Some(&home),
            &[],
        );
        assert_eq!(ran.code, 1, "{format}: {}", ran.stderr);
        assert!(ran.stderr.contains(&broken), "{format}: {}", ran.stderr);
        for path in [&c.repo, &other, &broken] {
            assert!(ran.stdout.contains(path.as_str()), "{format}: {path}");
        }
    }
    let report = status(&s, &["--repositories", &manifest, "--root", &src], 1);
    let problems: Vec<(String, Value)> = report["repositories"]
        .as_array()
        .expect("repositories")
        .iter()
        .map(|r| {
            (
                r["path"].as_str().unwrap_or_default().to_owned(),
                r["problem"]["code"].clone(),
            )
        })
        .collect();
    assert_eq!(
        problems,
        [
            (c.repo.clone(), Value::Null),
            (other.clone(), Value::from("no-default-branch")),
            (join(&src, "missing"), Value::from("skipped")),
            (broken.clone(), Value::from("unreadable")),
        ]
    );
    assert_eq!(
        worktree(&report["repositories"][0], &c.landed)["action"],
        "remove"
    );
    assert_eq!(report["totals"]["repositories"], 4);
    assert_eq!(report["totals"]["problems"], 3);
}

#[test]
fn test_a_manifest_of_clones_only_exits_0() {
    let s = Sandbox::new();
    let c = clone(&s);
    s.write(
        &join(&s.tmp, "repos.json"),
        r#"[{"path": "repo"}, {"path": "gone"}]"#,
    );
    let report = status(&s, &["--repositories", "repos.json", "--root", &s.tmp], 0);
    assert_eq!(report["repositories"][0]["path"], c.repo.as_str());
    assert_eq!(report["repositories"][1]["problem"]["code"], "skipped");
}

// A named target ------------------------------------------------------------------------

#[test]
fn test_a_named_target_narrows_every_format() {
    let s = Sandbox::new();
    let c = clone(&s);
    for name in ["landed", c.landed.as_str()] {
        let report = status(&s, &["--repository", &c.repo, name], 0);
        let repository = &report["repositories"][0];
        let paths: Vec<&Value> = repository["worktrees"]
            .as_array()
            .expect("worktrees")
            .iter()
            .map(|w| &w["path"])
            .collect();
        assert_eq!(paths, [&Value::from(c.landed.as_str())], "{name}");
        let names: Vec<&Value> = repository["branches"]
            .as_array()
            .expect("branches")
            .iter()
            .map(|b| &b["name"])
            .collect();
        assert_eq!(names, [&Value::from("landed")], "{name}");
        assert_eq!(repository["totals"]["worktrees.remove"], 1);
        assert_eq!(repository["totals"]["worktrees.keep"], 0);
        assert_eq!(report["totals"]["branches.delete"], 1);
    }
    for format in FORMATS {
        let ran = s.run(
            &[
                "status",
                "--repository",
                &c.repo,
                "--format",
                format,
                "landed",
            ],
            None,
            &[],
        );
        assert_eq!(ran.code, 0, "{format}: {}", ran.stderr);
        assert!(ran.stdout.contains(&c.landed), "{format}: {}", ran.stdout);
        for other in ["feat", "held", "gone"] {
            assert!(
                !ran.stdout.contains(other),
                "{format} names {other}: {}",
                ran.stdout
            );
        }
    }
}

#[test]
fn test_a_branch_name_brings_its_worktree() {
    let s = Sandbox::new();
    let c = clone(&s);
    let report = status(&s, &["--repository", &c.repo, "feat"], 0);
    let repository = &report["repositories"][0];
    assert_eq!(worktree(repository, &c.feat)["action"], "keep");
    assert_eq!(repository["worktrees"].as_array().map(Vec::len), Some(1));
    assert_eq!(repository["branches"].as_array().map(Vec::len), Some(1));
}

#[test]
fn test_an_unknown_name_is_bad_usage() {
    let s = Sandbox::new();
    let c = clone(&s);
    for format in FORMATS {
        let ran = s.run(
            &[
                "status",
                "--repository",
                &c.repo,
                "--format",
                format,
                "landed",
                "nowhere",
            ],
            None,
            &[],
        );
        assert_eq!((ran.code, ran.stdout.as_str()), (2, ""), "{format}");
        assert!(
            ran.stderr.contains("nowhere names no worktree or branch"),
            "{}",
            ran.stderr
        );
        assert!(!ran.stderr.contains("landed"), "{}", ran.stderr);
    }
}

/// A repository gitoxide finds and can't read: its packed refs are junk.
fn unreadable(s: &Sandbox, name: &str) -> String {
    let repo = s.repo_named(name, &[]);
    s.write(&join(&repo, ".git/packed-refs"), "not a packed ref\n");
    repo
}

#[test]
fn test_a_name_never_hides_an_unreadable_repository() {
    let s = Sandbox::new();
    clone(&s);
    let broken = unreadable(&s, "broken");
    let manifest = join(&s.tmp, "repos.json");
    s.write(&manifest, r#"[{"path": "repo"}, {"path": "broken"}]"#);
    for (args, name) in [
        (vec!["--repository", broken.as_str()], "feat"),
        (
            vec!["--repositories", &manifest, "--root", &s.tmp],
            "nowhere",
        ),
    ] {
        let named: Vec<&str> = args.iter().copied().chain([name]).collect();
        let report = status(&s, &named, 1);
        let problems: Vec<&Value> = report["repositories"]
            .as_array()
            .expect("repositories")
            .iter()
            .map(|r| &r["problem"]["code"])
            .collect();
        assert_eq!(problems, [&Value::from("unreadable")], "{args:?}");
        for format in FORMATS {
            let ran = s.run(
                &["status"]
                    .into_iter()
                    .chain(named.iter().copied())
                    .chain(["--format", format])
                    .collect::<Vec<&str>>(),
                None,
                &[],
            );
            assert_eq!(ran.code, 1, "{args:?} {format}: {}", ran.stderr);
            assert!(ran.stdout.contains(&broken), "{args:?} {format}");
            assert!(!ran.stderr.contains("names no"), "{}", ran.stderr);
        }
    }
}

#[test]
fn test_a_missing_manifest_fails_before_any_report() {
    let s = Sandbox::new();
    let missing = join(&s.tmp, "missing.json");
    let report = status(&s, &["--repositories", &missing], 1);
    assert_eq!(
        report,
        serde_json::json!({
            "schema": 1,
            "command": "status",
            "error": {"code": "unreadable", "detail": format!("{missing}: no such file")},
        })
    );
    let ran = s.run(&["status", "--repositories", &missing], None, &[]);
    assert_eq!((ran.code, ran.stdout.as_str()), (1, ""));
    assert!(ran.stderr.contains("no such file"), "{}", ran.stderr);
}

// Writing nothing ------------------------------------------------------------------------

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
fn test_status_writes_nothing() {
    let s = Sandbox::new();
    let c = clone(&s);
    s.write(&join(&c.feat, "notes.txt"), "uncommitted\n");
    // The same bytes at another time: a status that refreshed the index
    // would write it.
    for worktree in [&c.feat, &c.landed] {
        utime(&join(worktree, ".gitignore"), 1_000_000_000.0);
    }
    let manifest = join(&s.tmp, "repos.json");
    s.write(&manifest, r#"[{"path": "repo"}]"#);
    let state = join(&s.tmp, "state");
    fs::create_dir_all(&state).expect("the state folder");
    let before = snapshot(&s, &c.repo);
    for format in FORMATS {
        for args in [
            vec!["status", "--format", format],
            vec!["status", "--size", "--format", format, "feat"],
            vec![
                "status",
                "--repositories",
                &manifest,
                "--root",
                &s.tmp,
                "--format",
                format,
            ],
        ] {
            for cwd in [&c.repo, &c.feat] {
                let ran = s.run(&args, Some(cwd), &[]);
                assert_eq!(ran.code, 0, "{args:?}: {}", ran.stderr);
                assert_eq!(snapshot(&s, &c.repo), before, "{args:?} in {cwd} wrote");
            }
        }
    }
}
