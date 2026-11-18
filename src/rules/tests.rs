//! Tests of the sweep's pure rules: values in, values out, no files. Each
//! test has the name of its case in `worktrees/tests/test_rules.py`.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Local, TimeZone};
use serde_json::{Value, json};

use super::*;

const REF: &str = "refs/remotes/origin/main";

fn settings_fixture() -> Settings {
    Settings {
        sweep: SweepMode::Report,
        idle: 3600.0,
        lock_report: 86400.0,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn wt() -> WorktreeFacts {
    WorktreeFacts {
        path: "/w/repo.worktrees/feat".to_owned(),
        branch: Some("feat".to_owned()),
        head: "a".repeat(40),
        is_main: false,
        exists: true,
        lock: None,
        landed: Some(true),
        changes: Vec::new(),
        kept: Vec::new(),
        cleanable: Vec::new(),
        idle: Some(7200.0),
        broken: None,
    }
}

fn branch() -> BranchFacts {
    BranchFacts {
        name: "feat".to_owned(),
        tip: "a".repeat(40),
        upstream: None,
        track: String::new(),
        landed: Some(false),
        content_landed: Some(false),
        age: Some(2.0 * 86400.0),
        archived: false,
        is_default: false,
    }
}

fn lock_with(reason: &str, age: f64) -> LockFacts {
    LockFacts {
        lock: parse_lock(reason),
        age: Some(age),
        alive: None,
        process_start: None,
    }
}

fn lock() -> LockFacts {
    lock_with("neil since 2026-11-18T09:12:00Z", 10.0)
}

fn verdict(
    f: &WorktreeFacts,
    state: Option<BranchState>,
    problem: Option<&str>,
) -> WorktreeVerdict {
    worktree_verdict(f, state, Some(REF), problem, &settings_fixture())
}

fn codes(reasons: &[Reason]) -> Vec<ReasonCode> {
    reasons.iter().map(|r| r.code).collect()
}

fn event(value: Value) -> Event {
    match value {
        Value::Object(map) => map,
        _ => panic!("an event is an object"),
    }
}

mod times {
    use super::*;

    #[test]
    fn test_durations() {
        assert_eq!(parse_duration(Some("1h")), Some(3600.0));
        assert_eq!(parse_duration(Some("1d")), Some(86400.0));
        assert_eq!(parse_duration(Some("30m")), Some(1800.0));
        assert_eq!(parse_duration(Some("0s")), Some(0.0));
        assert_eq!(parse_duration(Some("90")), Some(90.0));
        assert_eq!(parse_duration(Some("soon")), None);
    }

    #[test]
    fn test_times() {
        assert_eq!(parse_time(Some("2026-11-18T09:12:00Z")), Some(1794993120.0));
        assert_eq!(parse_time(Some("1794993120000")), Some(1794993120.0));
        assert_eq!(parse_time(Some("1794993120")), Some(1794993120.0));
        assert_eq!(parse_time(Some("2026-11-18T09:12:00")), Some(1794993120.0));
        let local = Local
            .with_ymd_and_hms(2026, 11, 14, 16, 15, 55)
            .earliest()
            .map(|t| t.timestamp() as f64);
        assert_eq!(parse_time(Some("Sat Nov 14 16:15:55 2026")), local);
        assert_eq!(parse_time(Some("whenever")), None);
    }

    // Beyond the Python cases: ISO 8601 as Python 3.14, the shipped
    // package's interpreter, reads it, so a lock's `since` isn't lost.

    #[test]
    fn test_times_in_the_forms_python_3_14_reads() {
        assert_eq!(
            parse_time(Some("2026-11-18T09:12:00.5Z")),
            Some(1794993120.5)
        );
        assert_eq!(parse_time(Some("20261118T091200Z")), Some(1794993120.0));
        assert_eq!(
            parse_lock("neil since 20261118T091200Z").since,
            Some(1794993120.0)
        );
    }

    #[test]
    fn test_archive_refs_are_unique() {
        assert_eq!(
            archive_ref("a/b", 1794993120.0),
            "refs/archive/a/b/2026-11-18T091200Z"
        );
        assert_eq!(
            unique_refs(&["r", "r", "s"], &BTreeSet::from(["s".to_owned()])),
            strings(&["r", "r-2", "s-2"])
        );
    }

    #[test]
    fn test_backup_names() {
        let name = |url: Option<&str>, folder: &str| backup_name(url, folder);
        let pair = |owner: &str, repo: &str| (owner.to_owned(), repo.to_owned());

        assert_eq!(
            name(Some("git@github.com:nhooey/nix-worktrees.git"), "clone"),
            pair("nhooey", "nix-worktrees")
        );
        assert_eq!(
            name(Some("https://github.com/nhooey/ansible"), "clone"),
            pair("nhooey", "ansible")
        );
        assert_eq!(
            name(Some("ssh://git@github.com/nhooey/amqp.git/"), "clone"),
            pair("nhooey", "amqp")
        );
        assert_eq!(
            name(Some("file:///srv/git/xtdb.git"), "clone"),
            pair("git", "xtdb")
        );
        assert_eq!(
            name(Some("/tmp/x/repo.remote.git"), "clone"),
            pair("x", "repo.remote")
        );
        assert_eq!(name(None, "scratch"), pair("local", "scratch"));
        assert_eq!(name(Some(""), "scratch"), pair("local", "scratch"));
        assert_eq!(
            name(Some("https://github.com/../.."), "scratch"),
            pair("local", "scratch")
        );
        assert_eq!(name(Some("repo.git"), "scratch"), pair("local", "scratch"));
    }

    #[test]
    fn test_settings_from_git_config() {
        let defaults = json!({"sweep": "report", "idle": "1h", "lockReport": "1d"});
        let defaults = defaults.as_object().expect("an object");
        let s = settings(defaults, &parse_config("worktrees.idle 0s\n"));
        assert_eq!(
            s,
            Settings {
                sweep: SweepMode::Report,
                idle: 0.0,
                lock_report: 86400.0
            }
        );
        let config = |key: &str, value: &str| BTreeMap::from([(key.to_owned(), value.to_owned())]);
        assert_eq!(
            settings(defaults, &config("worktrees.sweep", "apply")).sweep,
            SweepMode::Apply
        );
        assert_eq!(
            settings(defaults, &config("worktrees.idle", "junk")).idle,
            3600.0
        );
    }
}

mod locks {
    use super::*;

    #[test]
    fn test_since_form() {
        let parsed = parse_lock("headless overnight2-03 pid 4411 since 2026-11-18T09:12:00Z");
        assert_eq!(parsed.form, LockForm::Since);
        assert_eq!(parsed.pid, Some(4411));
        assert_eq!(parsed.since, Some(1794993120.0));
        assert_eq!(parse_lock("neil since 2026-11-18T09:12:00Z").pid, None);
    }

    #[test]
    fn test_claude_form() {
        let parsed = parse_lock("claude agent agent-a1b2 (pid 93171 start 1794993120000)");
        assert_eq!(
            (parsed.form, parsed.pid, parsed.since),
            (LockForm::ClaudeAgent, Some(93171), Some(1794993120.0))
        );
        assert_eq!(parsed.who, "claude agent agent-a1b2");
    }

    #[test]
    fn test_other_form() {
        assert_eq!(parse_lock("busy").form, LockForm::Other);
        assert_eq!(parse_lock("").reason, "");
    }

    #[test]
    fn test_alive() {
        let claude = parse_lock("claude agent agent-a (pid 1 start 1000)");
        assert_eq!(process_alive(&claude, Some(1000.5)), Some(true));
        assert_eq!(
            process_alive(&claude, Some(2000.0)),
            Some(false),
            "a pid reused by a later process"
        );
        assert_eq!(process_alive(&claude, None), Some(false));
        let since = parse_lock("w pid 1 since 1970-01-01T00:16:40Z");
        assert_eq!(process_alive(&since, Some(900.0)), Some(true));
        assert_eq!(process_alive(&since, Some(1100.0)), Some(false));
        assert_eq!(
            process_alive(&parse_lock("neil since 2026-11-18T09:12:00Z"), None),
            None
        );
    }
}

mod patterns {
    use super::*;

    fn patterns(texts: &[&str]) -> Vec<Pattern> {
        texts.iter().map(|t| parse_pattern(t)).collect()
    }

    #[test]
    fn test_a_name_matches_at_any_depth() {
        let p = patterns(&[".env", "*.local.*"]);
        assert_eq!(
            kept_entries(&[".env", "a/.env", "x.local.toml", "build/"], &p),
            strings(&[".env", "a/.env", "x.local.toml"])
        );
    }

    #[test]
    fn test_folders_and_anchors() {
        let p = patterns(&[".superpowers/", ".claude/settings.local.json"]);
        assert_eq!(
            kept_entries(
                &[
                    ".superpowers/",
                    ".superpowers",
                    "a/.superpowers/x",
                    ".claude/settings.local.json",
                    "b/.claude/settings.local.json",
                ],
                &p,
            ),
            strings(&[
                ".superpowers/",
                "a/.superpowers/x",
                ".claude/settings.local.json"
            ])
        );
    }

    #[test]
    fn test_probes_inside_ignored_folders() {
        let p = patterns(&[".claude/settings.local.json", ".env", ".idea/"]);
        assert_eq!(
            probes(&[".claude/", ".idea/", "target/"], &p),
            strings(&[".claude/settings.local.json"])
        );
    }

    // Beyond the Python cases: stars match without backtracking.

    #[test]
    fn test_many_stars_match_at_once() {
        let stars = parse_pattern("*a*a*a*a*a*a*a*a*b");
        let started = std::time::Instant::now();
        assert!(!pattern_matches(&stars, &"a".repeat(60)));
        assert!(pattern_matches(&stars, &format!("{}b", "a".repeat(60))));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}

mod worktrees {
    use super::*;

    #[test]
    fn test_removable() {
        let v = verdict(&wt(), None, None);
        assert_eq!(
            (v.action, v.reasons.clone()),
            (WorktreeAction::Remove, vec![])
        );
        assert!(v.tests.iter().all(|t| t.ok));
    }

    #[test]
    fn test_each_rule_refuses_on_its_own() {
        let cases = [
            (
                WorktreeFacts {
                    is_main: true,
                    ..wt()
                },
                ReasonCode::MainWorktree,
            ),
            (
                WorktreeFacts {
                    lock: Some(lock()),
                    ..wt()
                },
                ReasonCode::Locked,
            ),
            (
                WorktreeFacts {
                    landed: Some(false),
                    ..wt()
                },
                ReasonCode::NotLanded,
            ),
            (
                WorktreeFacts {
                    changes: strings(&[" M a.txt"]),
                    ..wt()
                },
                ReasonCode::Uncommitted,
            ),
            (
                WorktreeFacts {
                    kept: strings(&[".env"]),
                    ..wt()
                },
                ReasonCode::KeepPatternFiles,
            ),
            (
                WorktreeFacts {
                    idle: Some(60.0),
                    ..wt()
                },
                ReasonCode::NotIdle,
            ),
        ];
        for (facts, code) in cases {
            let v = verdict(&facts, Some(BranchState::InReview), None);
            assert_eq!(
                (v.action, codes(&v.reasons)),
                (WorktreeAction::Keep, vec![code]),
                "{code}"
            );
        }
    }

    #[test]
    fn test_no_default_branch() {
        let v = verdict(
            &WorktreeFacts {
                landed: None,
                ..wt()
            },
            None,
            Some("no default branch"),
        );
        assert_eq!(codes(&v.reasons), vec![ReasonCode::NoDefaultBranch]);
    }

    #[test]
    fn test_content_landed_and_previews_count_as_landed() {
        let unlanded = WorktreeFacts {
            landed: Some(false),
            ..wt()
        };
        assert_eq!(
            verdict(&unlanded, Some(BranchState::ContentLanded), None).action,
            WorktreeAction::Remove
        );
        assert_eq!(
            verdict(&unlanded, Some(BranchState::Preview), None).action,
            WorktreeAction::Remove
        );
    }

    #[test]
    fn test_a_missing_folder_is_pruned_unless_locked() {
        let missing = WorktreeFacts {
            exists: false,
            ..wt()
        };
        assert_eq!(verdict(&missing, None, None).action, WorktreeAction::Prune);
        let locked = verdict(
            &WorktreeFacts {
                lock: Some(lock()),
                ..missing
            },
            None,
            None,
        );
        assert_eq!(
            (locked.action, codes(&locked.reasons)),
            (
                WorktreeAction::Keep,
                vec![ReasonCode::FolderMissing, ReasonCode::Locked]
            )
        );
    }

    #[test]
    fn test_an_unlanded_worktree_idle_a_day_is_cleaned() {
        let idle = WorktreeFacts {
            landed: Some(false),
            idle: Some(2.0 * 86400.0),
            cleanable: strings(&["target/"]),
            changes: strings(&["?? x"]),
            ..wt()
        };
        let local_only = Some(BranchState::LocalOnly);
        assert_eq!(
            verdict(&idle, local_only, None).action,
            WorktreeAction::Clean
        );
        assert_eq!(
            verdict(
                &WorktreeFacts {
                    lock: Some(lock()),
                    ..idle.clone()
                },
                local_only,
                None
            )
            .action,
            WorktreeAction::Keep
        );
        assert_eq!(
            verdict(
                &WorktreeFacts {
                    changes: strings(&[" M a"]),
                    ..idle.clone()
                },
                local_only,
                None
            )
            .action,
            WorktreeAction::Keep
        );
        assert_eq!(
            verdict(
                &WorktreeFacts {
                    cleanable: Vec::new(),
                    ..idle
                },
                local_only,
                None
            )
            .action,
            WorktreeAction::Keep
        );
    }

    #[test]
    fn test_a_landed_lock_held_past_the_limit_is_reported() {
        let reason = "neil since 2026-11-18T09:12:00Z";
        let locked = |age: f64, landed: bool| WorktreeFacts {
            lock: Some(lock_with(reason, age)),
            landed: Some(landed),
            ..wt()
        };
        assert!(verdict(&locked(2.0 * 86400.0, true), None, None).lock_report);
        assert!(!verdict(&locked(60.0, true), None, None).lock_report);
        assert!(!verdict(&locked(2.0 * 86400.0, false), None, None).lock_report);
    }
}

mod branches {
    use super::*;

    fn state(b: BranchFacts) -> BranchState {
        branch_state(&b, true)
    }

    #[test]
    fn test_the_table() {
        let upstream = Some("refs/remotes/origin/feat".to_owned());
        assert_eq!(
            state(BranchFacts {
                is_default: true,
                ..branch()
            }),
            BranchState::DefaultBranch
        );
        assert_eq!(branch_state(&branch(), false), BranchState::NoDefaultBranch);
        assert_eq!(
            state(BranchFacts {
                landed: Some(true),
                ..branch()
            }),
            BranchState::Landed
        );
        assert_eq!(
            state(BranchFacts {
                name: "preview/x".to_owned(),
                ..branch()
            }),
            BranchState::Preview
        );
        assert_eq!(
            state(BranchFacts {
                content_landed: Some(true),
                ..branch()
            }),
            BranchState::ContentLanded
        );
        assert_eq!(
            state(BranchFacts {
                upstream: upstream.clone(),
                track: "gone".to_owned(),
                ..branch()
            }),
            BranchState::UpstreamGone
        );
        assert_eq!(
            state(BranchFacts {
                upstream: upstream.clone(),
                track: "ahead 1, behind 2".to_owned(),
                ..branch()
            }),
            BranchState::Diverged
        );
        assert_eq!(
            state(BranchFacts {
                upstream,
                track: "ahead 1".to_owned(),
                ..branch()
            }),
            BranchState::InReview
        );
        assert_eq!(state(branch()), BranchState::LocalOnly);
    }

    #[test]
    fn test_actions() {
        let action = |survives: &[&str], b: BranchFacts| {
            branch_verdict(&b, true, Some(REF), None, None, strings(survives)).action
        };
        let preview = || BranchFacts {
            name: "preview/x".to_owned(),
            ..branch()
        };

        assert_eq!(
            action(
                &[],
                BranchFacts {
                    landed: Some(true),
                    ..branch()
                }
            ),
            BranchAction::Delete
        );
        assert_eq!(action(&[], preview()), BranchAction::ArchiveDelete);
        assert_eq!(
            action(&["pushed as origin/preview/x at aaa"], preview()),
            BranchAction::Delete
        );
        assert_eq!(
            action(
                &[],
                BranchFacts {
                    content_landed: Some(true),
                    ..branch()
                }
            ),
            BranchAction::ArchiveDelete
        );
        assert_eq!(action(&[], branch()), BranchAction::Keep);
        assert_eq!(
            action(&["pushed as origin/feat at aaa"], branch()),
            BranchAction::Keep
        );
    }

    #[test]
    fn test_where_a_tip_survives() {
        let refs: Vec<(String, String)> = [
            ("refs/heads/feat", "t"),
            ("refs/remotes/origin/feat", "t"),
            ("refs/remotes/origin/HEAD", "t"),
            ("refs/tags/v1", "t"),
            ("refs/heads/going", "t"),
            ("refs/heads/other", "u"),
        ]
        .iter()
        .map(|(r, c)| ((*r).to_owned(), (*c).to_owned()))
        .collect();
        assert_eq!(
            elsewhere("t", &refs, true, Some(REF)),
            strings(&["merged into origin/main", "pushed as origin/feat at t"])
        );
        assert_eq!(
            elsewhere("u", &refs, false, Some(REF)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn test_a_branch_in_a_kept_worktree_stays() {
        let landed = BranchFacts {
            landed: Some(true),
            ..branch()
        };
        let kept = branch_verdict(
            &landed,
            true,
            Some(REF),
            Some("/w/x"),
            Some(WorktreeAction::Keep),
            Vec::new(),
        );
        assert_eq!(
            (kept.action, codes(&kept.reasons)),
            (BranchAction::Keep, vec![ReasonCode::CheckedOut])
        );
        let freed = branch_verdict(
            &landed,
            true,
            Some(REF),
            Some("/w/x"),
            Some(WorktreeAction::Remove),
            Vec::new(),
        );
        assert_eq!(freed.action, BranchAction::Delete);
    }
}

mod content_landed {
    use super::*;

    #[test]
    fn test_raw_diff_and_tree() {
        let raw = ":100644 100644 aaa bbb M\0a.txt\0:100644 000000 ccc 000000 D\0gone.txt\0";
        let changes = parse_raw_diff(raw);
        let change = |path: &str, mode: &str, object: &str| Change {
            path: path.to_owned(),
            mode: mode.to_owned(),
            object: object.to_owned(),
        };
        assert_eq!(
            changes,
            vec![
                change("a.txt", "100644", "bbb"),
                change("gone.txt", "000000", "000000")
            ]
        );
        let entry = |mode: &str, object: &str| TreeEntry {
            mode: mode.to_owned(),
            object: object.to_owned(),
        };
        let tree = parse_ls_tree("100644 blob bbb\ta.txt\x00100644 blob ddd\tother.txt\x00");
        assert!(changes_present(&changes, tree.as_ref()));
        let with_gone = tree
            .clone()
            .into_iter()
            .flatten()
            .chain([("gone.txt".to_owned(), entry("100644", "ccc"))])
            .collect();
        assert!(!changes_present(&changes, Some(&with_gone)));
        assert!(!changes_present(
            &changes,
            Some(&BTreeMap::from([(
                "a.txt".to_owned(),
                entry("100644", "aaa")
            )]))
        ));
    }

    // Beyond the Python cases: a tree read wrongly must keep the branch.

    #[test]
    fn test_a_malformed_tree_is_not_content_landed() {
        let deletion = parse_raw_diff(":100644 000000 ccc 000000 D\0gone.txt\0");
        for text in [
            "100644 blob bbb\ta.txt\x00100644 blob\tgone.txt\x00",
            "100644 blob bbb\ta.txt\x00gone.txt\x00",
            "100644 blob bbb extra\tgone.txt\x00",
        ] {
            assert_eq!(parse_ls_tree(text), None, "{text:?}");
            assert!(!changes_present(&deletion, parse_ls_tree(text).as_ref()));
        }
        assert!(changes_present(
            &deletion,
            parse_ls_tree("100644 blob bbb\ta.txt\x00").as_ref()
        ));
    }
}

mod log {
    use super::*;

    #[test]
    fn test_rescue_takes_the_tip_from_the_deletion_or_the_last_value() {
        let zero = "0".repeat(40);
        let tip = |digit: &str| digit.repeat(40);
        let events: Vec<Event> = [
            json!({"event": "ref-update", "ref": "refs/heads/a", "new": tip("1")}),
            json!({"event": "ref-update", "ref": "refs/heads/a", "old": tip("1"), "new": tip("2")}),
            json!({"event": "ref-update", "ref": "refs/heads/a", "new": zero}),
            json!({"event": "ref-update", "ref": "refs/heads/b", "old": tip("3"), "new": zero}),
            json!({"event": "snapshot", "ref": "refs/heads/c", "new": tip("4")}),
            json!({"event": "ref-update", "ref": "refs/heads/c", "new": zero}),
            json!({"event": "ref-update", "ref": "refs/heads/preview/p", "old": tip("5"), "new": zero}),
            json!({"event": "ref-update", "ref": "refs/heads/never", "new": zero}),
            json!({"event": "ref-update", "ref": "refs/remotes/origin/x", "old": tip("6"), "new": zero}),
        ]
        .into_iter()
        .map(event)
        .collect();
        let rescue = |branch: &str, digit: &str| (branch.to_owned(), tip(digit));
        assert_eq!(
            rescue_candidates(&events),
            vec![
                rescue("a", "2"),
                rescue("b", "3"),
                rescue("c", "4"),
                rescue("preview/p", "5")
            ]
        );
    }

    #[test]
    fn test_backfill_only_once() {
        assert!(!has_snapshot(&[event(json!({"event": "remove"}))]));
        assert!(has_snapshot(&[event(json!({"event": "snapshot"}))]));
    }
}

mod plan {
    use super::*;

    fn repo() -> RepoFacts {
        RepoFacts {
            main: "/w/repo".to_owned(),
            common_dir: "/w/repo/.git".to_owned(),
            state_dir: "/s".to_owned(),
            default_branch: Some("main".to_owned()),
            default_ref: Some(REF.to_owned()),
            default_problem: None,
            settings: settings_fixture(),
            worktrees: vec![
                WorktreeFacts {
                    is_main: true,
                    path: "/w/repo".to_owned(),
                    branch: Some("main".to_owned()),
                    ..wt()
                },
                wt(),
            ],
            branches: vec![
                BranchFacts {
                    name: "main".to_owned(),
                    is_default: true,
                    landed: Some(true),
                    ..branch()
                },
                BranchFacts {
                    name: "feat".to_owned(),
                    landed: Some(true),
                    ..branch()
                },
                BranchFacts {
                    name: "rebased".to_owned(),
                    content_landed: Some(true),
                    tip: "b".repeat(40),
                    ..branch()
                },
                BranchFacts {
                    name: "open".to_owned(),
                    upstream: Some("refs/remotes/origin/open".to_owned()),
                    tip: "c".repeat(40),
                    ..branch()
                },
            ],
            archives: vec![(
                "refs/archive/old/2026-01-01T000000Z".to_owned(),
                "d".repeat(40),
            )],
            merged: BTreeSet::from(["refs/archive/old/2026-01-01T000000Z".to_owned()]),
            events: Vec::new(),
            rescues: vec![("doomed".to_owned(), "e".repeat(40))],
            refs: Vec::new(),
            backup: String::new(),
            backup_refs: Vec::new(),
        }
    }

    #[test]
    fn test_backfill_archives_every_unlanded_branch_first() {
        let facts = repo();
        let p = plan(&facts, 1794993120.0);
        assert!(p.backfill);
        assert_eq!(p.backfill_archives, strings(&["rebased", "open"]));
        assert_eq!(
            p.archive_for("rebased"),
            Some("refs/archive/rebased/2026-11-18T091200Z")
        );
        assert_eq!(
            p.worktrees.iter().map(|v| v.action).collect::<Vec<_>>(),
            vec![WorktreeAction::Keep, WorktreeAction::Remove]
        );
        assert_eq!(
            p.branches.iter().map(|v| v.action).collect::<Vec<_>>(),
            vec![
                BranchAction::Keep,
                BranchAction::Delete,
                BranchAction::ArchiveDelete,
                BranchAction::Keep
            ]
        );
        assert_eq!(
            p.archive_prunes,
            vec![(
                "refs/archive/old/2026-01-01T000000Z".to_owned(),
                "d".repeat(40)
            )]
        );
        assert_eq!(
            p.rescues,
            vec![Rescue {
                branch: "doomed".to_owned(),
                tip: "e".repeat(40),
                archive: "refs/archive/doomed/2026-11-18T091200Z".to_owned(),
            }]
        );
        assert_eq!(p.snapshots.len(), 2 + 4);
    }

    #[test]
    fn test_later_sweeps_snapshot_only_branches_missing_from_the_log() {
        let events = vec![
            event(json!({"event": "snapshot", "ref": "refs/heads/feat"})),
            event(json!({"event": "snapshot", "ref": "refs/heads/main"})),
        ];
        let facts = RepoFacts { events, ..repo() };
        let p = plan(&facts, 1794993120.0);
        assert!(!p.backfill);
        assert_eq!(p.backfill_archives, Vec::<String>::new());
        let sorted_refs = {
            let mut refs: Vec<Option<&str>> = p.snapshots.iter().map(Snapshot::reference).collect();
            refs.sort();
            refs
        };
        assert_eq!(
            sorted_refs,
            vec![Some("refs/heads/open"), Some("refs/heads/rebased")]
        );
        assert_eq!(
            p.archives,
            vec![(
                "rebased".to_owned(),
                "refs/archive/rebased/2026-11-18T091200Z".to_owned()
            )]
        );
    }

    #[test]
    fn test_no_default_branch_plans_nothing_but_rescues() {
        let facts = RepoFacts {
            default_problem: Some("no default branch".to_owned()),
            worktrees: vec![WorktreeFacts {
                landed: None,
                ..wt()
            }],
            ..repo()
        };
        let p = plan(&facts, 0.0);
        assert!(!p.backfill);
        assert_eq!(p.snapshots, Vec::new());
        assert_eq!(p.archive_prunes, Vec::new());
        assert!(p.worktrees.iter().all(|v| v.action == WorktreeAction::Keep));
        assert!(p.branches.iter().all(|v| v.action == BranchAction::Keep));
        assert_eq!(p.rescues.len(), 1);
    }
}
