//! The worktree tests, run in order, each failed one a reason; and the
//! worktree rules, which choose an action from their results.

use jiff::Timestamp;

use crate::lock;
use crate::model::Note;
use crate::model::facts::{LockFacts, RepositoryFacts, WorktreeFacts};
use crate::model::ids::{BranchState, ReasonCode, RuleId, TestName, WorktreeAction};
use crate::model::plan::WorktreePlan;
use crate::model::report::{LockReport, TestResult};
use crate::model::settings::Settings;
use crate::rules::{age, attention, survival};

/// One worktree, with what its tests read besides it.
#[derive(Clone, Copy, Debug)]
pub struct WorktreeSubject<'a> {
    pub worktree: &'a WorktreeFacts,
    pub repository: &'a RepositoryFacts,
    /// The state of the branch checked out; None when `HEAD` is detached
    /// or the branch has no commit.
    pub state: Option<BranchState>,
    pub settings: &'a Settings,
    pub now: Timestamp,
    /// `finish --abandon`'s reason, which passes the `landed` test of a
    /// worktree with a branch to archive.
    pub abandon: Option<&'a str>,
}

impl WorktreeSubject<'_> {
    /// Whether it is the main worktree; a bare repository has none.
    pub fn main(&self) -> bool {
        self.worktree.main && !self.repository.bare
    }
}

pub struct WorktreeTest {
    pub name: TestName,
    /// What passes it, in the node's words.
    pub passes: &'static str,
    /// The reason a failure gives.
    pub reason: ReasonCode,
    /// Whether it passes, and the values that decided it.
    pub run: fn(&WorktreeSubject<'_>) -> (bool, String),
}

/// The worktree tests, in the order they run.
pub static WORKTREE_TESTS: [WorktreeTest; 9] = [
    WorktreeTest {
        name: TestName::Folder,
        passes: "its folder exists",
        reason: ReasonCode::FolderMissing,
        run: |s| {
            if s.worktree.folder {
                (true, "its folder exists".to_owned())
            } else {
                let gone = format!("its folder {} is gone", s.worktree.path.display());
                (false, gone)
            }
        },
    },
    WorktreeTest {
        name: TestName::Working,
        passes: "the folder is a working worktree of this repository",
        reason: ReasonCode::Broken,
        run: |s| match &s.worktree.broken {
            None => (true, "a working worktree of this repository".to_owned()),
            Some(why) => (
                false,
                format!("not a working worktree of this repository: {why}"),
            ),
        },
    },
    WorktreeTest {
        name: TestName::NotMain,
        passes: "it is a linked worktree; a bare repository has no main worktree",
        reason: ReasonCode::MainWorktree,
        run: |s| {
            if s.main() {
                (false, "the main worktree".to_owned())
            } else {
                (true, "a linked worktree".to_owned())
            }
        },
    },
    WorktreeTest {
        name: TestName::DefaultBranch,
        passes: "the default branch is known",
        reason: ReasonCode::NoDefaultBranch,
        run: |s| match &s.repository.default {
            Some(d) => (
                true,
                format!("the default branch is {}, at {}", d.branch, d.reference),
            ),
            None => (false, "no default branch is known".to_owned()),
        },
    },
    WorktreeTest {
        name: TestName::Unlocked,
        passes: "it has no lock",
        reason: ReasonCode::Locked,
        run: |s| match &s.worktree.lock {
            None => (true, "not locked".to_owned()),
            Some(facts) => (false, lock_detail(facts)),
        },
    },
    WorktreeTest {
        name: TestName::Landed,
        passes: "its HEAD is merged, or its branch is disposable or content-landed",
        reason: ReasonCode::NotLanded,
        run: landed,
    },
    WorktreeTest {
        name: TestName::Clean,
        passes: "nothing is uncommitted, tracked or untracked",
        reason: ReasonCode::Uncommitted,
        run: |s| match s.worktree.uncommitted.as_slice() {
            [] => (true, "nothing uncommitted".to_owned()),
            paths => (
                false,
                format!("{} uncommitted paths: {}", paths.len(), first_five(paths)),
            ),
        },
    },
    WorktreeTest {
        name: TestName::KeepFiles,
        passes: "it holds no kept file of its own",
        reason: ReasonCode::KeepPatternFiles,
        run: |s| match s.worktree.own_files.as_slice() {
            [] => (true, "no kept files of its own".to_owned()),
            paths => (
                false,
                format!(
                    "{} kept files of its own: {}",
                    paths.len(),
                    first_five(paths)
                ),
            ),
        },
    },
    WorktreeTest {
        name: TestName::Idle,
        passes: "its HEAD, logs/HEAD and index have been unchanged for `idle`",
        reason: ReasonCode::NotIdle,
        run: |s| match s.worktree.changed {
            None => (
                false,
                "its HEAD, logs/HEAD and index couldn't be read".to_owned(),
            ),
            Some(changed) => {
                let idle = s.now.duration_since(changed);
                (
                    idle >= s.settings.idle,
                    format!("idle {} of the {} needed", age(idle), age(s.settings.idle)),
                )
            }
        },
    },
];

/// How the worktree's work has landed, whatever `--abandon` says; None
/// when it hasn't.
pub fn landed_work(s: &WorktreeSubject<'_>) -> Option<String> {
    let merged = s.worktree.head.as_ref().and_then(|h| h.merged);
    match (merged, s.state) {
        (Some(true), _) => Some(format!(
            "HEAD is merged into {}",
            s.repository
                .default
                .as_ref()
                .map_or("", |d| d.reference.as_str())
        )),
        (_, Some(BranchState::Disposable)) => Some("its branch is disposable".to_owned()),
        (_, Some(BranchState::ContentLanded)) => Some("its branch's content has landed".to_owned()),
        _ => None,
    }
}

fn landed(s: &WorktreeSubject<'_>) -> (bool, String) {
    // Only a branch is archived, so abandoning a detached HEAD would lose it.
    let abandoned = s.abandon.filter(|_| s.worktree.branch.is_some());
    match (landed_work(s), abandoned) {
        (Some(how), _) => (true, how),
        (None, Some(reason)) => (true, format!("abandoned: {reason}")),
        (None, None) => (
            false,
            match (&s.worktree.head, &s.repository.default) {
                (None, _) => "its HEAD can't be read".to_owned(),
                (_, None) => "unknown, with no default branch to measure against".to_owned(),
                (Some(_), Some(d)) => format!(
                    "HEAD is not merged into {}{}",
                    d.reference,
                    s.state.map_or(String::new(), |state| format!(" ({state})"))
                ),
            },
        ),
    }
}

fn lock_detail(facts: &LockFacts) -> String {
    let reason = match facts.lock.reason.as_str() {
        "" => "(no reason)",
        reason => reason,
    };
    let liveness = match (
        facts.lock.pid,
        lock::alive(&facts.lock, facts.process_start),
    ) {
        (Some(pid), Some(true)) => format!("; pid {pid} is alive"),
        (Some(pid), Some(false)) => format!("; pid {pid} is gone"),
        _ => String::new(),
    };
    format!("locked: {reason}{liveness}")
}

fn first_five(paths: &[std::path::PathBuf]) -> String {
    let shown: Vec<String> = paths
        .iter()
        .take(5)
        .map(|p| p.display().to_string())
        .collect();
    let more = if paths.len() > 5 { ", …" } else { "" };
    format!("{}{more}", shown.join(", "))
}

/// Whether the test `name` ran and passed.
pub fn passed(tests: &[TestResult], name: TestName) -> bool {
    tests.iter().any(|t| t.name == name && t.ok)
}

pub struct WorktreeRule {
    pub id: RuleId,
    /// When it applies, in the node's words.
    pub when: &'static str,
    pub action: WorktreeAction,
    pub applies: fn(&[TestResult]) -> bool,
}

/// The worktree rules, in the order they are tried. The last applies to
/// every worktree.
pub static WORKTREE_RULES: [WorktreeRule; 3] = [
    WorktreeRule {
        id: RuleId::WorktreeRemove,
        when: "every test passes",
        action: WorktreeAction::Remove,
        applies: |tests| tests.iter().all(|t| t.ok),
    },
    WorktreeRule {
        id: RuleId::WorktreePrune,
        when: "the folder is gone, and it is unlocked and linked; a HEAD neither merged nor \
               archived is archived first, and pruned only once that archive passed its compare",
        action: WorktreeAction::Prune,
        applies: |tests| {
            !passed(tests, TestName::Folder)
                && passed(tests, TestName::Unlocked)
                && passed(tests, TestName::NotMain)
        },
    },
    WorktreeRule {
        id: RuleId::WorktreeKeep,
        when: "anything else",
        action: WorktreeAction::Keep,
        applies: |_| true,
    },
];

/// The worktree's plan: every test but `skip`, in order, and the first
/// rule their results meet. A kept worktree has a reason for each failed
/// test.
pub fn plan(subject: &WorktreeSubject<'_>, skip: Option<TestName>) -> WorktreePlan {
    let w = subject.worktree;
    let run: Vec<(&WorktreeTest, TestResult)> = WORKTREE_TESTS
        .iter()
        .filter(|test| Some(test.name) != skip)
        .map(|test| {
            let (ok, detail) = (test.run)(subject);
            let result = TestResult {
                name: test.name,
                ok,
                detail,
            };
            (test, result)
        })
        .collect();
    let tests: Vec<TestResult> = run.iter().map(|(_, result)| result.clone()).collect();
    let rule = WORKTREE_RULES
        .iter()
        .find(|rule| (rule.applies)(&tests))
        .unwrap_or(&WORKTREE_RULES[WORKTREE_RULES.len() - 1]);
    let reasons = match rule.action {
        WorktreeAction::Keep => run
            .iter()
            .filter(|(_, result)| !result.ok)
            .map(|(test, result)| Note::new(test.reason, result.detail.clone()))
            .collect(),
        WorktreeAction::Remove | WorktreeAction::Prune => Vec::new(),
    };
    WorktreePlan {
        path: w.path.clone(),
        branch: w.branch.clone(),
        head: w.head.as_ref().map(|h| h.oid),
        main: subject.main(),
        action: rule.action,
        rule: rule.id,
        attention: attention::worktree(subject, &tests),
        tests,
        reasons,
        survives: w
            .head
            .as_ref()
            .map(|h| survival::survives(h, subject.repository))
            .unwrap_or_default(),
        lock: w.lock.as_ref().map(|facts| LockReport {
            reason: facts.lock.reason.clone(),
            who: facts.lock.who.clone(),
            pid: facts.lock.pid,
            since: facts.lock.since,
            alive: lock::alive(&facts.lock, facts.process_start),
        }),
        bytes: w.bytes,
    }
}
