//! The rules over facts built in code, with no repository: each branch
//! rule and worktree test, their order, the plan they compose, `finish`'s
//! plan, properties over generated facts, and the catalogue `rules`
//! prints.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};

use git_worktree_manager::backup;
use git_worktree_manager::cli;
use git_worktree_manager::lock;
use git_worktree_manager::model::Oid;
use git_worktree_manager::model::facts::{
    ArchiveFacts, BackupFacts, BranchFacts, CommitFacts, DefaultFacts, LockFacts, RefFacts,
    RepositoryFacts, UpstreamFacts, WorktreeFacts,
};
use git_worktree_manager::model::ids::{
    AttentionCode, BranchAction, BranchState, DefaultSource, ReasonCode, RuleId, SurvivalKind,
    TestName, WorktreeAction,
};
use git_worktree_manager::model::plan::{BranchPlan, Plan, WorktreePlan};
use git_worktree_manager::model::settings::Settings;
use git_worktree_manager::rules;
use git_worktree_manager::rules::prune::{self, PruneArchive};
use git_worktree_manager::system::Fake;

// Facts ---------------------------------------------------------------------------

const NOW: &str = "2026-11-19T09:12:00Z";
const DEFAULT_REF: &str = "refs/remotes/origin/main";

fn now() -> Timestamp {
    NOW.parse().expect("a time")
}

fn ago(hours: i64) -> Timestamp {
    now() - SignedDuration::from_hours(hours)
}

fn oid(digit: char) -> Oid {
    Oid::from_hex(&digit.to_string().repeat(40)).expect("an object name")
}

fn commit(digit: char, merged: Option<bool>) -> CommitFacts {
    CommitFacts {
        oid: oid(digit),
        time: ago(48),
        merged,
    }
}

fn repository() -> RepositoryFacts {
    RepositoryFacts {
        path: "/w/repo".into(),
        common_dir: "/w/repo/.git".into(),
        bare: false,
        default: Some(DefaultFacts {
            branch: "main".to_owned(),
            reference: DEFAULT_REF.to_owned(),
            oid: oid('d'),
            source: DefaultSource::OriginHead,
        }),
        worktrees: Vec::new(),
        branches: Vec::new(),
        remote_refs: Vec::new(),
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: Vec::new(),
        },
    }
}

/// A repository with no default branch: every commit's ancestry unknown.
fn without_default(facts: RepositoryFacts) -> RepositoryFacts {
    let unknown = |c: CommitFacts| CommitFacts { merged: None, ..c };
    RepositoryFacts {
        default: None,
        worktrees: facts
            .worktrees
            .into_iter()
            .map(|w| WorktreeFacts {
                head: w.head.map(unknown),
                ..w
            })
            .collect(),
        branches: facts
            .branches
            .into_iter()
            .map(|b| BranchFacts {
                tip: unknown(b.tip),
                content_landed: None,
                ..b
            })
            .collect(),
        ..facts
    }
}

fn worktree_path(name: &str) -> PathBuf {
    PathBuf::from("/w/repo.worktrees").join(name)
}

/// A linked worktree on `branch` that passes every test.
fn worktree(branch: &str) -> WorktreeFacts {
    WorktreeFacts {
        path: worktree_path(branch),
        main: false,
        folder: true,
        broken: None,
        branch: Some(branch.to_owned()),
        head: Some(commit('a', Some(true))),
        lock: None,
        uncommitted: Vec::new(),
        own_files: Vec::new(),
        changed: Some(ago(2)),
        bytes: None,
    }
}

fn main_worktree() -> WorktreeFacts {
    WorktreeFacts {
        path: "/w/repo".into(),
        main: true,
        ..worktree("main")
    }
}

/// A local-only branch, two days old, neither merged nor landed.
fn branch(name: &str) -> BranchFacts {
    BranchFacts {
        name: name.to_owned(),
        tip: commit('a', Some(false)),
        upstream: None,
        content_landed: Some(false),
    }
}

fn upstream(name: &str, gone: bool, ahead: u64, behind: u64) -> Option<UpstreamFacts> {
    Some(UpstreamFacts {
        reference: format!("refs/remotes/origin/{name}"),
        gone,
        ahead,
        behind,
    })
}

fn locked(reason: &str, process_start: Option<Timestamp>) -> Option<LockFacts> {
    Some(LockFacts {
        lock: lock::parse(reason, &TimeZone::UTC),
        written: Some(ago(1)),
        process_start,
    })
}

fn archive(branch: &str, digit: char, compared: Option<bool>) -> ArchiveFacts {
    ArchiveFacts {
        reference: format!("refs/archive/{branch}/2026-01-01T000000Z"),
        branch: branch.to_owned(),
        time: "2026-01-01T00:00:00Z".parse().expect("a time"),
        oid: oid(digit),
        compared,
    }
}

fn with(worktrees: Vec<WorktreeFacts>, branches: Vec<BranchFacts>) -> RepositoryFacts {
    RepositoryFacts {
        worktrees,
        branches,
        ..repository()
    }
}

fn plan(facts: &RepositoryFacts) -> Plan {
    rules::plan(facts, &Settings::default(), now())
}

/// The plan of the one branch `facts` holds, with no worktree.
fn branch_plan(b: BranchFacts) -> BranchPlan {
    plan(&with(vec![], vec![b])).branches.remove(0)
}

/// The plan of the one linked worktree `facts` holds, on a local-only
/// branch of its name.
fn worktree_plan(w: WorktreeFacts) -> WorktreePlan {
    plan(&with(vec![w], vec![])).worktrees.remove(0)
}

fn reasons<C: Copy>(notes: &[git_worktree_manager::model::Note<C>]) -> Vec<C> {
    notes.iter().map(|n| n.code).collect()
}

// Branch rules --------------------------------------------------------------------

#[test]
fn test_each_branch_rule() {
    let cases = [
        (
            branch("main"),
            RuleId::BranchDefault,
            BranchState::DefaultBranch,
            BranchAction::Keep,
            vec![ReasonCode::DefaultBranch],
        ),
        (
            BranchFacts {
                tip: commit('a', Some(true)),
                ..branch("fix")
            },
            RuleId::BranchLanded,
            BranchState::Landed,
            BranchAction::Delete,
            vec![],
        ),
        (
            branch("preview/x"),
            RuleId::BranchDisposable,
            BranchState::Disposable,
            BranchAction::Delete,
            vec![],
        ),
        (
            BranchFacts {
                content_landed: Some(true),
                ..branch("rebased")
            },
            RuleId::BranchContentLanded,
            BranchState::ContentLanded,
            BranchAction::Delete,
            vec![],
        ),
        (
            BranchFacts {
                upstream: upstream("gone", true, 0, 0),
                ..branch("gone")
            },
            RuleId::BranchUpstreamGone,
            BranchState::UpstreamGone,
            BranchAction::Keep,
            vec![ReasonCode::UpstreamGone],
        ),
        (
            BranchFacts {
                upstream: upstream("both", false, 1, 2),
                ..branch("both")
            },
            RuleId::BranchDiverged,
            BranchState::Diverged,
            BranchAction::Keep,
            vec![ReasonCode::Diverged],
        ),
        (
            BranchFacts {
                upstream: upstream("open", false, 1, 0),
                ..branch("open")
            },
            RuleId::BranchInReview,
            BranchState::InReview,
            BranchAction::Keep,
            vec![ReasonCode::InReview],
        ),
        (
            branch("wip"),
            RuleId::BranchLocalOnly,
            BranchState::LocalOnly,
            BranchAction::Keep,
            vec![ReasonCode::LocalOnly],
        ),
    ];
    for (facts, rule, state, action, codes) in cases {
        let name = facts.name.clone();
        let p = branch_plan(facts);
        assert_eq!(
            (p.rule, p.state, p.action, reasons(&p.reasons)),
            (rule, state, action, codes),
            "{name}"
        );
    }
    let p = plan(&without_default(with(vec![], vec![branch("main")])));
    assert_eq!(
        (p.branches[0].rule, p.branches[0].action),
        (RuleId::BranchNoDefault, BranchAction::Keep)
    );
}

#[test]
fn test_the_first_branch_rule_matched_decides() {
    let everything = BranchFacts {
        tip: commit('a', Some(true)),
        content_landed: Some(true),
        upstream: upstream("x", true, 1, 1),
        ..branch("preview/x")
    };
    let cases = [
        (
            BranchFacts {
                name: "main".to_owned(),
                ..everything.clone()
            },
            RuleId::BranchDefault,
        ),
        (everything.clone(), RuleId::BranchLanded),
        (
            BranchFacts {
                tip: commit('a', Some(false)),
                ..everything.clone()
            },
            RuleId::BranchDisposable,
        ),
        (
            BranchFacts {
                tip: commit('a', Some(false)),
                name: "x".to_owned(),
                ..everything.clone()
            },
            RuleId::BranchContentLanded,
        ),
        (
            BranchFacts {
                upstream: upstream("x", true, 1, 1),
                ..branch("x")
            },
            RuleId::BranchUpstreamGone,
        ),
        (
            BranchFacts {
                upstream: upstream("x", false, 1, 1),
                ..branch("x")
            },
            RuleId::BranchDiverged,
        ),
        (
            BranchFacts {
                upstream: upstream("x", false, 0, 3),
                ..branch("x")
            },
            RuleId::BranchInReview,
        ),
    ];
    for (facts, rule) in cases {
        assert_eq!(branch_plan(facts.clone()).rule, rule, "{facts:?}");
    }
    let unknown = plan(&without_default(with(vec![], vec![everything])));
    assert_eq!(unknown.branches[0].rule, RuleId::BranchNoDefault);
}

#[test]
fn test_disposable_patterns_in_git_glob_syntax() {
    let state = |name: &str, patterns: &[&str]| {
        let settings = Settings {
            disposable: patterns.iter().map(|p| (*p).to_owned()).collect(),
            ..Settings::default()
        };
        rules::plan(&with(vec![], vec![branch(name)]), &settings, now()).branches[0].state
    };
    assert_eq!(state("preview/x", &["preview/*"]), BranchState::Disposable);
    assert_eq!(
        state("preview/a/b", &["preview/*"]),
        BranchState::Disposable
    );
    assert_eq!(state("previews", &["preview/*"]), BranchState::LocalOnly);
    assert_eq!(
        state("tmp-1", &["preview/*", "tmp-?"]),
        BranchState::Disposable
    );
    assert_eq!(state("preview/x", &[]), BranchState::LocalOnly);
}

// Where commits survive, and archives ---------------------------------------------------

#[test]
fn test_where_a_tip_survives() {
    let facts = RepositoryFacts {
        remote_refs: vec![
            RefFacts {
                name: "refs/remotes/origin/fix".to_owned(),
                oid: oid('a'),
            },
            RefFacts {
                name: "refs/remotes/origin/other".to_owned(),
                oid: oid('b'),
            },
        ],
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: vec![
                archive("fix", 'a', Some(true)),
                archive("old", 'a', None),
                archive("bad", 'a', Some(false)),
            ],
        },
        ..with(
            vec![],
            vec![BranchFacts {
                tip: commit('a', Some(true)),
                ..branch("fix")
            }],
        )
    };
    let survives: Vec<(SurvivalKind, String)> = plan(&facts).branches[0]
        .survives
        .iter()
        .map(|s| (s.kind, s.reference.clone()))
        .collect();
    assert_eq!(
        survives,
        [
            (SurvivalKind::Merged, DEFAULT_REF.to_owned()),
            (SurvivalKind::Pushed, "refs/remotes/origin/fix".to_owned()),
            (
                SurvivalKind::Archived,
                "refs/archive/fix/2026-01-01T000000Z".to_owned()
            ),
        ]
    );
}

#[test]
fn test_archive_then_delete_names_the_archive_unless_the_tip_is_archived() {
    let disposable = |archives: Vec<ArchiveFacts>, remote: bool| {
        let facts = RepositoryFacts {
            backup: BackupFacts {
                path: "/b/repo.git".into(),
                archives,
            },
            remote_refs: if remote {
                vec![RefFacts {
                    name: "refs/remotes/origin/preview/x".to_owned(),
                    oid: oid('a'),
                }]
            } else {
                vec![]
            },
            ..with(vec![], vec![branch("preview/x")])
        };
        plan(&facts).branches.remove(0)
    };
    let fresh = Some("refs/archive/preview/x/2026-11-19T091200Z".to_owned());
    assert_eq!(disposable(vec![], false).archive, fresh);
    // Pushed proves nothing: the ref is as old as the last fetch.
    let pushed = disposable(vec![], true);
    assert_eq!(
        (pushed.action, pushed.archive),
        (BranchAction::Delete, fresh.clone())
    );
    // An archive that hasn't passed the compare pass proves nothing either.
    assert_eq!(
        disposable(vec![archive("preview/x", 'a', None)], false).archive,
        fresh
    );
    let archived = disposable(vec![archive("preview/x", 'a', Some(true))], false);
    assert_eq!(
        (archived.action, archived.archive),
        (BranchAction::Delete, None)
    );
    // A landed branch is merged, so it needs no archive.
    let landed = branch_plan(BranchFacts {
        tip: commit('a', Some(true)),
        ..branch("fix")
    });
    assert_eq!(
        (landed.action, landed.archive),
        (BranchAction::Delete, None)
    );
}

#[test]
fn test_archive_names_are_unique() {
    let taken = ArchiveFacts {
        reference: "refs/archive/preview/x/2026-11-19T091200Z".to_owned(),
        ..archive("preview/x", 'f', Some(true))
    };
    let facts = RepositoryFacts {
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: vec![taken],
        },
        ..with(vec![], vec![branch("preview/x"), branch("preview/y")])
    };
    let names: Vec<Option<String>> = plan(&facts)
        .branches
        .into_iter()
        .map(|b| b.archive)
        .collect();
    assert_eq!(
        names,
        [
            Some("refs/archive/preview/x/2026-11-19T091200Z-2".to_owned()),
            Some("refs/archive/preview/y/2026-11-19T091200Z".to_owned()),
        ]
    );
}

#[test]
fn test_a_branch_in_a_worktree_that_stays_is_kept() {
    let landed = BranchFacts {
        tip: commit('a', Some(true)),
        ..branch("fix")
    };
    let kept = plan(&with(
        vec![WorktreeFacts {
            uncommitted: vec!["a.txt".into()],
            ..worktree("fix")
        }],
        vec![landed.clone()],
    ));
    let b = &kept.branches[0];
    assert_eq!(
        (b.rule, b.action, reasons(&b.reasons), b.archive.clone()),
        (
            RuleId::BranchLanded,
            BranchAction::Keep,
            vec![ReasonCode::CheckedOut],
            None
        )
    );
    assert_eq!(b.worktree.as_deref(), Some(worktree_path("fix").as_path()));
    let freed = plan(&with(vec![worktree("fix")], vec![landed.clone()]));
    assert_eq!(
        (freed.worktrees[0].action, freed.branches[0].action),
        (WorktreeAction::Remove, BranchAction::Delete)
    );
    let pruned = plan(&with(
        vec![WorktreeFacts {
            folder: false,
            ..worktree("fix")
        }],
        vec![landed],
    ));
    assert_eq!(
        (pruned.worktrees[0].action, pruned.branches[0].action),
        (WorktreeAction::Prune, BranchAction::Delete)
    );
    // A kept branch gives both reasons, the worktree's first.
    let wip = plan(&with(
        vec![WorktreeFacts {
            head: Some(commit('a', Some(false))),
            ..worktree("wip")
        }],
        vec![branch("wip")],
    ));
    assert_eq!(
        reasons(&wip.branches[0].reasons),
        [ReasonCode::CheckedOut, ReasonCode::LocalOnly]
    );
}

// Worktree tests ------------------------------------------------------------------

#[test]
fn test_a_worktree_passing_every_test_is_removed() {
    let p = worktree_plan(worktree("fix"));
    assert_eq!(
        (p.action, p.rule, p.reasons.len()),
        (WorktreeAction::Remove, RuleId::WorktreeRemove, 0)
    );
    let names: Vec<TestName> = p.tests.iter().map(|t| t.name).collect();
    assert_eq!(names, TestName::ALL);
    assert!(p.tests.iter().all(|t| t.ok), "{:?}", p.tests);
    assert_eq!(p.tests[8].detail, "idle 2h of the 1h needed");
    assert_eq!(p.survives[0].kind, SurvivalKind::Merged);
}

#[test]
fn test_each_worktree_test_refuses_on_its_own() {
    let base = worktree("fix");
    let cases = [
        (
            WorktreeFacts {
                broken: Some("its .git file points elsewhere".to_owned()),
                ..base.clone()
            },
            TestName::Working,
            ReasonCode::Broken,
        ),
        (
            WorktreeFacts {
                main: true,
                ..base.clone()
            },
            TestName::NotMain,
            ReasonCode::MainWorktree,
        ),
        (
            WorktreeFacts {
                lock: locked("neil since 2026-11-18T09:12:00Z", None),
                ..base.clone()
            },
            TestName::Unlocked,
            ReasonCode::Locked,
        ),
        (
            WorktreeFacts {
                head: Some(commit('a', Some(false))),
                ..base.clone()
            },
            TestName::Landed,
            ReasonCode::NotLanded,
        ),
        (
            WorktreeFacts {
                uncommitted: vec!["a.txt".into()],
                ..base.clone()
            },
            TestName::Clean,
            ReasonCode::Uncommitted,
        ),
        (
            WorktreeFacts {
                own_files: vec![".env".into()],
                ..base.clone()
            },
            TestName::KeepFiles,
            ReasonCode::KeepPatternFiles,
        ),
        (
            WorktreeFacts {
                changed: Some(now() - SignedDuration::from_mins(5)),
                ..base.clone()
            },
            TestName::Idle,
            ReasonCode::NotIdle,
        ),
        (
            WorktreeFacts {
                changed: None,
                ..base.clone()
            },
            TestName::Idle,
            ReasonCode::NotIdle,
        ),
    ];
    for (facts, test, code) in cases {
        let p = worktree_plan(facts);
        let failed: Vec<TestName> = p.tests.iter().filter(|t| !t.ok).map(|t| t.name).collect();
        assert_eq!(
            (p.action, failed, reasons(&p.reasons)),
            (WorktreeAction::Keep, vec![test], vec![code]),
            "{test}"
        );
    }
}

#[test]
fn test_failed_tests_give_reasons_in_the_tests_order() {
    let p = worktree_plan(WorktreeFacts {
        changed: Some(now()),
        own_files: vec![".env".into()],
        uncommitted: vec!["a".into()],
        lock: locked("busy", None),
        ..worktree("fix")
    });
    assert_eq!(
        reasons(&p.reasons),
        [
            ReasonCode::Locked,
            ReasonCode::Uncommitted,
            ReasonCode::KeepPatternFiles,
            ReasonCode::NotIdle
        ]
    );
}

#[test]
fn test_without_a_default_branch_nothing_is_removed() {
    let facts = without_default(with(
        vec![main_worktree(), worktree("fix")],
        vec![branch("main"), branch("fix")],
    ));
    let p = plan(&facts);
    assert_eq!(
        p.problem.as_ref().map(|n| n.code),
        Some(git_worktree_manager::model::ids::ProblemCode::NoDefaultBranch)
    );
    assert_eq!(
        reasons(&p.worktrees[1].reasons),
        [ReasonCode::NoDefaultBranch, ReasonCode::NotLanded]
    );
    assert!(p.worktrees.iter().all(|w| w.action == WorktreeAction::Keep));
    assert!(
        p.branches
            .iter()
            .all(|b| b.action == BranchAction::Keep && b.rule == RuleId::BranchNoDefault)
    );
}

#[test]
fn test_disposable_and_content_landed_branches_count_as_landed() {
    let unmerged = |name: &str| WorktreeFacts {
        head: Some(commit('a', Some(false))),
        ..worktree(name)
    };
    let p = plan(&with(
        vec![unmerged("preview/x"), unmerged("rebased")],
        vec![
            branch("preview/x"),
            BranchFacts {
                content_landed: Some(true),
                ..branch("rebased")
            },
        ],
    ));
    assert!(
        p.worktrees
            .iter()
            .all(|w| w.action == WorktreeAction::Remove)
    );
    assert!(
        p.branches
            .iter()
            .all(|b| b.action == BranchAction::Delete && b.archive.is_some())
    );
}

#[test]
fn test_a_missing_folder_is_pruned_unless_locked_or_main() {
    let gone = WorktreeFacts {
        folder: false,
        ..worktree("fix")
    };
    assert_eq!(worktree_plan(gone.clone()).rule, RuleId::WorktreePrune);
    let locked = worktree_plan(WorktreeFacts {
        lock: locked("neil since 2026-11-18T09:12:00Z", None),
        ..gone.clone()
    });
    assert_eq!(
        (locked.rule, reasons(&locked.reasons)),
        (
            RuleId::WorktreeKeep,
            vec![ReasonCode::FolderMissing, ReasonCode::Locked]
        )
    );
    let main = worktree_plan(WorktreeFacts {
        main: true,
        ..gone.clone()
    });
    assert_eq!(main.action, WorktreeAction::Keep);
}

/// A worktree whose folder is gone, detached at `digit`'s commit.
fn gone_detached(name: &str, digit: char, merged: Option<bool>) -> WorktreeFacts {
    WorktreeFacts {
        folder: false,
        branch: None,
        head: Some(commit(digit, merged)),
        ..worktree(name)
    }
}

/// The plan, and the archives its prunes need.
fn prunes(facts: &RepositoryFacts) -> (Plan, Vec<PruneArchive>) {
    let p = plan(facts);
    let archives = prune::prune_archives(&p, facts, now());
    (p, archives)
}

fn refs(archives: &[PruneArchive]) -> Vec<&str> {
    archives.iter().map(|a| a.reference.as_str()).collect()
}

fn verified(references: &[&str]) -> BTreeSet<String> {
    references.iter().map(|r| (*r).to_owned()).collect()
}

#[test]
fn test_a_detached_head_is_archived_before_its_prune() {
    let (p, archives) = prunes(&with(
        vec![gone_detached("loose", 'a', Some(false))],
        vec![],
    ));
    assert_eq!(p.worktrees[0].rule, RuleId::WorktreePrune);
    let name = "refs/archive/detached-head/loose/2026-11-19T091200Z";
    assert_eq!(
        archives,
        [PruneArchive {
            worktree: worktree_path("loose"),
            oid: oid('a'),
            reference: name.to_owned(),
        }]
    );
    assert_eq!(
        backup::parse_archive_ref(name),
        Some(("detached-head/loose".to_owned(), now()))
    );
    assert_eq!(
        prune::prunable(&p, &archives, &verified(&[])),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        prune::prunable(&p, &archives, &verified(&[name])),
        [worktree_path("loose")]
    );
    assert_eq!(
        prune::detached_head_name(Path::new("/w/my repo.v2")),
        "detached-head/my-repo-v2"
    );
}

#[test]
fn test_a_merged_or_archived_head_is_pruned_with_no_archive() {
    let merged = with(
        vec![WorktreeFacts {
            folder: false,
            ..worktree("fix")
        }],
        vec![BranchFacts {
            tip: commit('a', Some(true)),
            ..branch("fix")
        }],
    );
    let archived = RepositoryFacts {
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: vec![archive("old", 'a', Some(true))],
        },
        ..with(vec![gone_detached("loose", 'a', Some(false))], vec![])
    };
    for facts in [merged, archived] {
        let (p, archives) = prunes(&facts);
        assert_eq!(
            (p.worktrees[0].rule, archives.len()),
            (RuleId::WorktreePrune, 0)
        );
        assert_eq!(
            prune::prunable(&p, &archives, &verified(&[])),
            [p.worktrees[0].path.clone()]
        );
    }
}

#[test]
fn test_a_failed_archive_keeps_its_worktree_registered() {
    let (p, archives) = prunes(&with(
        vec![
            gone_detached("failed", 'a', Some(false)),
            gone_detached("passed", 'b', Some(false)),
        ],
        vec![],
    ));
    assert_eq!(
        refs(&archives),
        [
            "refs/archive/detached-head/failed/2026-11-19T091200Z",
            "refs/archive/detached-head/passed/2026-11-19T091200Z",
        ]
    );
    assert_eq!(
        prune::prunable(&p, &archives, &verified(&[archives[1].reference.as_str()])),
        [worktree_path("passed")]
    );
}

#[test]
fn test_a_prune_archive_is_its_branch_s_and_one_per_commit() {
    // The disposable branch's plan names an archive; its pruned worktree,
    // and a detached one at the same commit, reuse it.
    let taken = ArchiveFacts {
        reference: "refs/archive/wip/2026-11-19T091200Z".to_owned(),
        ..archive("wip", 'f', Some(true))
    };
    let facts = RepositoryFacts {
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: vec![taken],
        },
        ..with(
            vec![
                WorktreeFacts {
                    folder: false,
                    head: Some(commit('a', Some(false))),
                    ..worktree("preview/x")
                },
                gone_detached("same", 'a', Some(false)),
                WorktreeFacts {
                    folder: false,
                    head: Some(commit('b', Some(false))),
                    ..worktree("wip")
                },
            ],
            vec![
                branch("preview/x"),
                BranchFacts {
                    tip: commit('b', Some(false)),
                    ..branch("wip")
                },
            ],
        )
    };
    let (p, archives) = prunes(&facts);
    let branch_archive = p.branches[0].archive.clone().expect("an archive");
    assert_eq!(
        refs(&archives),
        [
            branch_archive.as_str(),
            branch_archive.as_str(),
            "refs/archive/wip/2026-11-19T091200Z-2",
        ]
    );
}

#[test]
fn test_without_a_default_branch_a_missing_folder_is_archived_and_pruned() {
    let facts = without_default(with(
        vec![WorktreeFacts {
            folder: false,
            ..worktree("wip")
        }],
        vec![branch("wip")],
    ));
    let (p, archives) = prunes(&facts);
    assert_eq!(p.worktrees[0].rule, RuleId::WorktreePrune);
    assert_eq!(refs(&archives), ["refs/archive/wip/2026-11-19T091200Z"]);
    assert_eq!(p.branches[0].action, BranchAction::Keep);
}

#[test]
fn test_a_locked_missing_folder_is_kept_and_needs_no_archive() {
    let (p, archives) = prunes(&with(
        vec![WorktreeFacts {
            lock: locked("busy", None),
            ..gone_detached("loose", 'a', Some(false))
        }],
        vec![],
    ));
    assert_eq!(
        (p.worktrees[0].rule, archives.len()),
        (RuleId::WorktreeKeep, 0)
    );
    assert_eq!(
        prune::prunable(&p, &archives, &verified(&[])),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn test_a_lock_is_never_broken_and_its_liveness_reported() {
    let reason = "claude agent agent-a1b2 (pid 93171 start 1794993120000)";
    let started: Timestamp = "2026-11-18T09:12:00Z".parse().expect("a time");
    let cases = [
        (Some(started), Some(true), "; pid 93171 is alive"),
        (None, Some(false), "; pid 93171 is gone"),
    ];
    for (process_start, alive, said) in cases {
        let p = worktree_plan(WorktreeFacts {
            lock: locked(reason, process_start),
            ..worktree("fix")
        });
        let report = p.lock.expect("a lock");
        assert_eq!(
            (p.action, report.pid, report.alive),
            (WorktreeAction::Keep, Some(93171), alive)
        );
        assert!(
            p.reasons[0].detail.ends_with(said),
            "{}",
            p.reasons[0].detail
        );
    }
}

// Attention -----------------------------------------------------------------------

#[test]
fn test_worktree_attention() {
    let attention = |w: WorktreeFacts| reasons(&worktree_plan(w).attention);
    assert_eq!(attention(worktree("fix")), []);
    assert_eq!(
        attention(WorktreeFacts {
            uncommitted: vec!["a".into()],
            ..worktree("fix")
        }),
        [AttentionCode::Uncommitted]
    );
    assert_eq!(
        attention(WorktreeFacts {
            uncommitted: vec!["a".into()],
            ..main_worktree()
        }),
        []
    );
    let old_lock = Some(LockFacts {
        written: Some(ago(49)),
        ..locked("busy", None).expect("a lock")
    });
    assert_eq!(
        attention(WorktreeFacts {
            lock: old_lock.clone(),
            ..worktree("fix")
        }),
        [AttentionCode::LockedLanded]
    );
    assert_eq!(
        attention(WorktreeFacts {
            lock: locked("busy", None),
            ..worktree("fix")
        }),
        []
    );
    assert_eq!(
        attention(WorktreeFacts {
            lock: old_lock,
            head: Some(commit('a', Some(false))),
            ..worktree("fix")
        }),
        []
    );
    assert_eq!(
        attention(WorktreeFacts {
            broken: Some("gone astray".to_owned()),
            ..worktree("fix")
        }),
        [AttentionCode::Broken]
    );
}

#[test]
fn test_branch_attention() {
    let attention = |b: BranchFacts| reasons(&branch_plan(b).attention);
    assert_eq!(attention(branch("wip")), []);
    assert_eq!(
        attention(BranchFacts {
            tip: CommitFacts {
                time: ago(24 * 8),
                ..commit('a', Some(false))
            },
            ..branch("wip")
        }),
        [AttentionCode::LocalOnlyOld]
    );
    assert_eq!(
        attention(BranchFacts {
            upstream: upstream("x", false, 1, 1),
            ..branch("x")
        }),
        [AttentionCode::Diverged]
    );
    assert_eq!(
        attention(BranchFacts {
            upstream: upstream("x", true, 0, 0),
            ..branch("x")
        }),
        [AttentionCode::UpstreamGone]
    );
    let failed = RepositoryFacts {
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: vec![archive("preview/x", 'a', Some(false))],
        },
        ..with(vec![], vec![branch("preview/x")])
    };
    assert_eq!(
        reasons(&plan(&failed).branches[0].attention),
        [AttentionCode::ArchiveFailed]
    );
}

/// `limit` exactly, and one second either side of it.
fn around(limit: SignedDuration) -> [(SignedDuration, &'static str); 3] {
    let second = SignedDuration::from_secs(1);
    [
        (limit - second, "under"),
        (limit, "at"),
        (limit + second, "over"),
    ]
}

#[test]
fn test_idle_passes_at_its_limit() {
    let settings = Settings::default();
    for (idle, at, ok) in around(settings.idle)
        .into_iter()
        .zip([false, true, true])
        .map(|((d, at), ok)| (d, at, ok))
    {
        let p = worktree_plan(WorktreeFacts {
            changed: Some(now() - idle),
            ..worktree("fix")
        });
        let test = p.tests.iter().find(|t| t.name == TestName::Idle);
        assert_eq!(test.map(|t| t.ok), Some(ok), "{at}");
    }
}

#[test]
fn test_lock_report_needs_attention_past_its_limit() {
    let settings = Settings::default();
    for (held, at, attention) in around(settings.lock_report)
        .into_iter()
        .zip([vec![], vec![], vec![AttentionCode::LockedLanded]])
        .map(|((d, at), a)| (d, at, a))
    {
        let p = worktree_plan(WorktreeFacts {
            lock: Some(LockFacts {
                written: Some(now() - held),
                ..locked("busy", None).expect("a lock")
            }),
            ..worktree("fix")
        });
        assert_eq!(reasons(&p.attention), attention, "{at}");
    }
}

#[test]
fn test_local_only_report_needs_attention_past_its_limit() {
    let settings = Settings::default();
    for (old, at, attention) in around(settings.local_only_report)
        .into_iter()
        .zip([vec![], vec![], vec![AttentionCode::LocalOnlyOld]])
        .map(|((d, at), a)| (d, at, a))
    {
        let p = branch_plan(BranchFacts {
            tip: CommitFacts {
                time: now() - old,
                ..commit('a', Some(false))
            },
            ..branch("wip")
        });
        assert_eq!(reasons(&p.attention), attention, "{at}");
    }
}

// The plan ------------------------------------------------------------------------

#[test]
fn test_a_repository_planned_whole() {
    let facts = with(
        vec![main_worktree(), worktree("feat")],
        vec![
            BranchFacts {
                tip: commit('d', Some(true)),
                ..branch("main")
            },
            BranchFacts {
                tip: commit('a', Some(true)),
                ..branch("feat")
            },
            BranchFacts {
                tip: commit('b', Some(false)),
                content_landed: Some(true),
                ..branch("rebased")
            },
            BranchFacts {
                tip: commit('c', Some(false)),
                upstream: upstream("open", false, 1, 0),
                ..branch("open")
            },
        ],
    );
    let p = plan(&facts);
    let worktrees: Vec<(WorktreeAction, bool)> =
        p.worktrees.iter().map(|w| (w.action, w.main)).collect();
    assert_eq!(
        worktrees,
        [
            (WorktreeAction::Keep, true),
            (WorktreeAction::Remove, false)
        ]
    );
    assert_eq!(reasons(&p.worktrees[0].reasons), [ReasonCode::MainWorktree]);
    let branches: Vec<(BranchAction, Option<&str>)> = p
        .branches
        .iter()
        .map(|b| (b.action, b.archive.as_deref()))
        .collect();
    assert_eq!(
        branches,
        [
            (BranchAction::Keep, None),
            (BranchAction::Delete, None),
            (
                BranchAction::Delete,
                Some("refs/archive/rebased/2026-11-19T091200Z")
            ),
            (BranchAction::Keep, None),
        ]
    );
    assert_eq!(p.backup, Path::new("/b/repo.git"));
    assert_eq!(p.default.as_ref().map(|d| d.oid), Some(oid('d')));
    assert_eq!(p.problem, None);
}

#[test]
fn test_a_bare_repository_has_no_main_worktree() {
    let facts = RepositoryFacts {
        path: "/w/repo.git".into(),
        common_dir: "/w/repo.git".into(),
        bare: true,
        // A reader that marks one main is overruled: a bare repository has
        // none.
        ..with(
            vec![
                worktree("fix"),
                WorktreeFacts {
                    main: true,
                    ..worktree("main")
                },
            ],
            vec![branch("main"), branch("fix")],
        )
    };
    let p = plan(&facts);
    assert!(p.worktrees.iter().all(|w| !w.main));
    assert!(
        p.worktrees
            .iter()
            .all(|w| w.action == WorktreeAction::Remove)
    );
    assert_eq!(p.branches[0].action, BranchAction::Keep);
}

// Finish --------------------------------------------------------------------------

#[test]
fn test_finish_skips_the_idle_test() {
    let busy = with(
        vec![
            main_worktree(),
            WorktreeFacts {
                changed: Some(now()),
                ..worktree("fix")
            },
        ],
        vec![BranchFacts {
            tip: commit('a', Some(true)),
            ..branch("fix")
        }],
    );
    assert_eq!(plan(&busy).worktrees[1].action, WorktreeAction::Keep);
    let p = rules::finish(
        &busy,
        &Settings::default(),
        now(),
        &worktree_path("fix"),
        None,
    )
    .expect("the worktree");
    assert_eq!(p.worktrees.len(), 1);
    assert!(
        p.worktrees[0]
            .tests
            .iter()
            .all(|t| t.name != TestName::Idle)
    );
    assert_eq!(p.worktrees[0].action, WorktreeAction::Remove);
    assert_eq!(
        p.branches.iter().map(|b| b.action).collect::<Vec<_>>(),
        [BranchAction::Delete]
    );
    assert_eq!(
        rules::finish(
            &busy,
            &Settings::default(),
            now(),
            Path::new("/elsewhere"),
            None
        ),
        None
    );
}

#[test]
fn test_finish_keeps_a_worktree_with_a_commit_of_its_own() {
    let facts = with(
        vec![WorktreeFacts {
            head: Some(commit('a', Some(false))),
            ..worktree("wip")
        }],
        vec![branch("wip")],
    );
    let p = rules::finish(
        &facts,
        &Settings::default(),
        now(),
        &worktree_path("wip"),
        None,
    )
    .expect("the worktree");
    assert_eq!(reasons(&p.worktrees[0].reasons), [ReasonCode::NotLanded]);
    assert_eq!(p.branches[0].action, BranchAction::Keep);
}

#[test]
fn test_finish_abandon_passes_landed_and_archives_the_branch() {
    let facts = with(
        vec![WorktreeFacts {
            head: Some(commit('a', Some(false))),
            ..worktree("wip")
        }],
        vec![BranchFacts {
            upstream: upstream("wip", false, 1, 0),
            ..branch("wip")
        }],
    );
    let finish = |facts: &RepositoryFacts, name: &str| {
        rules::finish(
            facts,
            &Settings::default(),
            now(),
            &worktree_path(name),
            Some("superseded"),
        )
        .expect("the worktree")
    };
    let p = finish(&facts, "wip");
    let landed = p.worktrees[0]
        .tests
        .iter()
        .find(|t| t.name == TestName::Landed)
        .expect("the landed test");
    assert_eq!(
        (landed.ok, landed.detail.as_str()),
        (true, "abandoned: superseded")
    );
    assert_eq!(p.worktrees[0].action, WorktreeAction::Remove);
    let b = &p.branches[0];
    assert_eq!(
        (b.action, b.archive.as_deref(), b.reasons.len()),
        (
            BranchAction::Delete,
            Some("refs/archive/wip/2026-11-19T091200Z"),
            0
        )
    );
    // Uncommitted changes still keep it, and its branch with it, for that
    // reason alone.
    let dirty = with(
        vec![WorktreeFacts {
            uncommitted: vec!["a".into()],
            ..facts.worktrees[0].clone()
        }],
        facts.branches.clone(),
    );
    let p = finish(&dirty, "wip");
    assert_eq!(reasons(&p.worktrees[0].reasons), [ReasonCode::Uncommitted]);
    assert_eq!(reasons(&p.branches[0].reasons), [ReasonCode::CheckedOut]);
    // The default branch is never abandoned.
    let default = with(vec![worktree("main")], vec![branch("main")]);
    let p = finish(&default, "main");
    assert_eq!(p.worktrees[0].action, WorktreeAction::Remove);
    assert_eq!(p.branches[0].action, BranchAction::Keep);
    // A lock is never broken, abandoned or not, and raises no attention
    // for a worktree that hasn't landed.
    let held = with(
        vec![WorktreeFacts {
            lock: Some(LockFacts {
                written: Some(ago(24 * 30)),
                ..locked("busy", None).expect("a lock")
            }),
            ..facts.worktrees[0].clone()
        }],
        facts.branches.clone(),
    );
    let p = finish(&held, "wip");
    assert_eq!(
        (p.worktrees[0].action, reasons(&p.worktrees[0].reasons)),
        (WorktreeAction::Keep, vec![ReasonCode::Locked])
    );
    assert_eq!(reasons(&p.worktrees[0].attention), []);
    assert_eq!(reasons(&p.branches[0].reasons), [ReasonCode::CheckedOut]);
}

#[test]
fn test_finish_abandon_keeps_an_unmerged_detached_head() {
    // Only a branch is archived, so a detached HEAD can't be abandoned.
    let detached = |merged| {
        with(
            vec![WorktreeFacts {
                branch: None,
                head: Some(commit('a', Some(merged))),
                ..worktree("loose")
            }],
            vec![],
        )
    };
    let finish = |facts: &RepositoryFacts| {
        rules::finish(
            facts,
            &Settings::default(),
            now(),
            &worktree_path("loose"),
            Some("superseded"),
        )
        .expect("the worktree")
    };
    let p = finish(&detached(false));
    assert_eq!(
        (p.worktrees[0].action, reasons(&p.worktrees[0].reasons)),
        (WorktreeAction::Keep, vec![ReasonCode::NotLanded])
    );
    assert_eq!(
        finish(&detached(true)).worktrees[0].action,
        WorktreeAction::Remove
    );
}

// Properties over generated facts -------------------------------------------------

/// SplitMix64: the `n`th of a seed's pseudo-random numbers.
fn draw(seed: u64, n: u64) -> u64 {
    let z = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(n.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    let z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const NAMES: [&str; 5] = ["main", "fix", "preview/x", "wip", "open"];

fn pick<T: Clone>(seed: u64, n: u64, options: &[T]) -> T {
    options[(draw(seed, n) % options.len() as u64) as usize].clone()
}

fn maybe(seed: u64, n: u64) -> bool {
    draw(seed, n).is_multiple_of(3)
}

fn generated(seed: u64) -> RepositoryFacts {
    let digits = ['a', 'b', 'c'];
    let merged = |n: u64| pick(seed, n, &[Some(true), Some(false)]);
    let branches: Vec<BranchFacts> = NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| *i == 0 || !maybe(seed, 10 + *i as u64))
        .map(|(i, name)| {
            let n = 100 * (i as u64 + 1);
            BranchFacts {
                tip: CommitFacts {
                    oid: oid(pick(seed, n, &digits)),
                    time: ago(pick(seed, n + 1, &[1, 30, 400])),
                    merged: merged(n + 2),
                },
                upstream: maybe(seed, n + 3).then(|| UpstreamFacts {
                    reference: format!("refs/remotes/origin/{name}"),
                    gone: maybe(seed, n + 4),
                    ahead: draw(seed, n + 5) % 2,
                    behind: draw(seed, n + 6) % 2,
                }),
                content_landed: Some(maybe(seed, n + 7)),
                ..branch(name)
            }
        })
        .collect();
    let bare = maybe(seed, 1);
    let linked = (draw(seed, 2) % 4) as usize;
    let worktrees: Vec<WorktreeFacts> = (!bare)
        .then(main_worktree)
        .into_iter()
        .chain((0..linked).map(|i| {
            let n = 1000 * (i as u64 + 1);
            let on: BranchFacts = pick(seed, n, &branches);
            let detached = maybe(seed, n + 3);
            let detached_head = (!maybe(seed, n + 10)).then(|| CommitFacts {
                oid: oid(pick(seed, n + 4, &digits)),
                time: ago(5),
                merged: merged(n + 5),
            });
            WorktreeFacts {
                path: worktree_path(&format!("{}-{i}", on.name)),
                // A reader may mark a bare repository's worktree main; the
                // plan must not.
                main: bare && maybe(seed, n + 11),
                folder: !maybe(seed, n + 1),
                broken: maybe(seed, n + 2).then(|| "astray".to_owned()),
                branch: (!detached).then(|| on.name.clone()),
                head: if detached {
                    detached_head
                } else {
                    Some(on.tip.clone())
                },
                lock: maybe(seed, n + 6)
                    .then(|| locked("busy pid 7 since 2026-11-18T09:12:00Z", None))
                    .flatten(),
                uncommitted: if maybe(seed, n + 7) {
                    vec!["a".into()]
                } else {
                    vec![]
                },
                own_files: if maybe(seed, n + 8) {
                    vec![".env".into()]
                } else {
                    vec![]
                },
                changed: Some(ago(pick(seed, n + 9, &[0, 2, 60]))),
                ..worktree(&on.name)
            }
        }))
        .collect();
    let facts = RepositoryFacts {
        bare,
        worktrees,
        branches,
        remote_refs: vec![RefFacts {
            name: "refs/remotes/origin/wip".to_owned(),
            oid: oid(pick(seed, 3, &digits)),
        }],
        backup: BackupFacts {
            path: "/b/repo.git".into(),
            archives: vec![archive(
                "fix",
                pick(seed, 4, &digits),
                pick(seed, 5, &[None, Some(true), Some(false)]),
            )],
        },
        ..repository()
    };
    if maybe(seed, 6) {
        without_default(facts)
    } else {
        facts
    }
}

/// Every plan the generator makes, beside its facts and the repository's
/// whole plan: that plan itself, and `finish` on each of its worktrees,
/// with and without `--abandon`.
fn generated_plans() -> Vec<(RepositoryFacts, Plan, Plan)> {
    (0..3000)
        .map(generated)
        .flat_map(|facts| {
            let full = plan(&facts);
            let finishes: Vec<Plan> = facts
                .worktrees
                .iter()
                .flat_map(|w| {
                    [None, Some("dropped")].map(|abandon| {
                        rules::finish(&facts, &Settings::default(), now(), &w.path, abandon)
                            .expect("a worktree of the facts")
                    })
                })
                .collect();
            std::iter::once(full.clone())
                .chain(finishes)
                .map(move |p| (facts.clone(), full.clone(), p))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn keeps_commits(survives: &[git_worktree_manager::model::report::Survival]) -> bool {
    survives
        .iter()
        .any(|s| matches!(s.kind, SurvivalKind::Merged | SurvivalKind::Archived))
}

#[test]
fn test_properties_of_every_generated_plan() {
    for (facts, full, p) in generated_plans() {
        let archives = prune::prune_archives(&p, &facts, now());
        // A `finish` plan holds one worktree; the others keep their plan.
        let action_of = |path: &Path| {
            p.worktrees
                .iter()
                .chain(&full.worktrees)
                .find(|w| w.path == path)
                .map(|w| w.action)
        };
        let branch_of = |name: &str| {
            p.branches
                .iter()
                .chain(&full.branches)
                .find(|b| b.name == name)
        };
        for b in p
            .branches
            .iter()
            .filter(|b| b.action == BranchAction::Delete)
        {
            assert!(keeps_commits(&b.survives) || b.archive.is_some(), "{b:?}");
            assert_ne!(b.state, BranchState::DefaultBranch, "{b:?}");
            let stays = facts
                .worktrees
                .iter()
                .filter(|f| f.branch.as_deref() == Some(b.name.as_str()))
                .any(|f| action_of(&f.path) == Some(WorktreeAction::Keep));
            assert!(!stays, "{b:?}");
        }
        for w in &p.worktrees {
            let wf = facts
                .worktrees
                .iter()
                .find(|f| f.path == w.path)
                .expect("a worktree of the facts");
            assert_eq!(w.main, wf.main && !facts.bare, "{w:?}");
            if w.action == WorktreeAction::Keep {
                assert!(!w.reasons.is_empty(), "{w:?}");
            } else {
                assert!(wf.lock.is_none() && !w.main, "{w:?}");
                assert!(w.reasons.is_empty(), "{w:?}");
            }
            if w.action == WorktreeAction::Remove {
                let branch_holds = w
                    .branch
                    .as_deref()
                    .and_then(branch_of)
                    .is_some_and(|b| b.action == BranchAction::Keep || b.archive.is_some());
                assert!(keeps_commits(&w.survives) || branch_holds, "{w:?}");
                assert!(
                    wf.folder && wf.uncommitted.is_empty() && wf.own_files.is_empty(),
                    "{w:?}"
                );
            }
            if w.action == WorktreeAction::Prune {
                assert!(!wf.folder, "{w:?}");
                let named = archives
                    .iter()
                    .any(|a| a.worktree == w.path && Some(a.oid) == w.head);
                assert!(
                    w.head.is_none() || keeps_commits(&w.survives) || named,
                    "{w:?}"
                );
            }
        }
        if facts.default.is_none() {
            assert!(
                p.worktrees
                    .iter()
                    .all(|w| w.action != WorktreeAction::Remove)
            );
            assert!(p.branches.iter().all(|b| b.action == BranchAction::Keep));
        }
    }
}

// The catalogue -------------------------------------------------------------------

fn rules_command(format: &str) -> String {
    let mut system = Fake::new(now());
    let args = ["git-worktree-manager", "rules", "--format", format].map(OsString::from);
    let status = cli::run(args, &mut system);
    assert_eq!((status.code(), system.stderr.as_str()), (0, ""));
    system.stdout
}

fn strings(json: &serde_json::Value, list: &str, key: &str) -> BTreeSet<String> {
    json[list]
        .as_array()
        .unwrap_or_else(|| panic!("{list} is a list"))
        .iter()
        .map(|entry| entry[key].as_str().expect("a string").to_owned())
        .collect()
}

#[test]
fn test_rules_json_lists_everything_any_plan_names() {
    let json: serde_json::Value = serde_json::from_str(&rules_command("json")).expect("JSON");
    assert_eq!(
        (json["schema"].as_u64(), json["command"].as_str()),
        (Some(1), Some("rules"))
    );
    let listed_rules: BTreeSet<String> = strings(&json, "branch_rules", "id")
        .into_iter()
        .chain(strings(&json, "worktree_rules", "id"))
        .collect();
    let listed_tests = strings(&json, "worktree_tests", "name");
    let listed_reasons = strings(&json, "reasons", "code");
    let listed_attention = strings(&json, "attention", "code");

    let plans = generated_plans();
    let named = |of: &dyn Fn(&Plan) -> Vec<String>| -> BTreeSet<String> {
        plans.iter().flat_map(|(_, _, p)| of(p)).collect()
    };
    let named_rules = named(&|p| {
        p.worktrees
            .iter()
            .map(|w| w.rule.to_string())
            .chain(p.branches.iter().map(|b| b.rule.to_string()))
            .collect()
    });
    let named_tests = named(&|p| {
        p.worktrees
            .iter()
            .flat_map(|w| w.tests.iter().map(|t| t.name.to_string()))
            .collect()
    });
    let named_reasons = named(&|p| {
        p.worktrees
            .iter()
            .flat_map(|w| w.reasons.iter().map(|r| r.code.to_string()))
            .chain(
                p.branches
                    .iter()
                    .flat_map(|b| b.reasons.iter().map(|r| r.code.to_string())),
            )
            .collect()
    });
    let named_attention = named(&|p| {
        p.worktrees
            .iter()
            .flat_map(|w| w.attention.iter().map(|a| a.code.to_string()))
            .chain(
                p.branches
                    .iter()
                    .flat_map(|b| b.attention.iter().map(|a| a.code.to_string())),
            )
            .collect()
    });
    // The generator reaches every rule and test, so the check means
    // something.
    assert_eq!(named_rules.len(), RuleId::ALL.len(), "{named_rules:?}");
    assert_eq!(named_tests.len(), TestName::ALL.len());
    assert!(named_rules.is_subset(&listed_rules), "{named_rules:?}");
    assert!(named_tests.is_subset(&listed_tests));
    assert!(
        named_reasons.is_subset(&listed_reasons),
        "{named_reasons:?}"
    );
    assert!(
        named_attention.is_subset(&listed_attention),
        "{named_attention:?}"
    );

    let every = |all: Vec<String>| all.into_iter().collect::<BTreeSet<String>>();
    assert_eq!(
        listed_rules,
        every(RuleId::ALL.iter().map(ToString::to_string).collect())
    );
    assert_eq!(
        listed_tests,
        every(TestName::ALL.iter().map(ToString::to_string).collect())
    );
    assert_eq!(
        listed_reasons,
        every(ReasonCode::ALL.iter().map(ToString::to_string).collect())
    );
    assert_eq!(
        listed_attention,
        every(AttentionCode::ALL.iter().map(ToString::to_string).collect())
    );
}

#[test]
fn test_rules_text_follows_the_node() {
    let text = rules_command("text");
    for line in [
        "  1. branch.default, default-branch: it is the default branch. Keep (default-branch).",
        "  4. branch.disposable, disposable: its name matches a disposable pattern. Archive, then delete.",
        "  9. idle: its HEAD, logs/HEAD and index have been unchanged for `idle`. Else not-idle.",
        "  2. worktree.prune: the folder is gone, and it is unlocked and linked; a HEAD neither \
         merged nor archived is archived first, and pruned only once that archive passed its \
         compare. Prune.",
        "  checked-out: the branch is checked out in a worktree that stays.",
        "  locked-landed, on a worktree: a landed worktree locked longer than lockReport.",
    ] {
        assert!(text.lines().any(|l| l == line), "{line}\n\n{text}");
    }
}
