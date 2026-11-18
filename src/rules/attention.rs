//! What a person must settle, as codes on a worktree or a branch.

use crate::model::Note;
use crate::model::facts::LockFacts;
use crate::model::ids::{AttentionCode, BranchState, TestName};
use crate::model::report::TestResult;
use crate::rules::age;
use crate::rules::branch::BranchSubject;
use crate::rules::worktree::{WorktreeSubject, landed_work};

pub struct WorktreeAttention {
    pub code: AttentionCode,
    /// When it applies, in the node's words.
    pub when: &'static str,
    /// The detail when it applies, given the worktree's test results.
    pub check: fn(&WorktreeSubject<'_>, &[TestResult]) -> Option<String>,
}

pub struct BranchAttention {
    pub code: AttentionCode,
    /// When it applies, in the node's words.
    pub when: &'static str,
    /// The detail when it applies, given the branch's state.
    pub check: fn(&BranchSubject<'_>, BranchState) -> Option<String>,
}

pub static WORKTREE_ATTENTION: [WorktreeAttention; 3] = [
    WorktreeAttention {
        code: AttentionCode::Uncommitted,
        when: "a linked worktree with uncommitted changes",
        check: |s, _| {
            let count = s.worktree.uncommitted.len();
            (!s.main() && count > 0).then(|| format!("{count} uncommitted paths"))
        },
    },
    WorktreeAttention {
        code: AttentionCode::LockedLanded,
        when: "a landed worktree locked longer than lockReport",
        // Landed by its work, not by `--abandon`, which passes the test.
        check: |s, _| {
            let locked = s.worktree.lock.as_ref().and_then(|facts| {
                locked_since(facts).map(|since| (facts, s.now.duration_since(since)))
            });
            locked
                .filter(|(_, held)| landed_work(s).is_some() && *held > s.settings.lock_report)
                .map(|(facts, held)| {
                    format!(
                        "landed, and locked by {} for {}, past the {} allowed",
                        facts.lock.who,
                        age(held),
                        age(s.settings.lock_report)
                    )
                })
        },
    },
    WorktreeAttention {
        code: AttentionCode::Broken,
        when: "a worktree that fails `working`",
        check: |_, tests| {
            tests
                .iter()
                .find(|t| t.name == TestName::Working && !t.ok)
                .map(|t| t.detail.clone())
        },
    },
];

pub static BRANCH_ATTENTION: [BranchAttention; 4] = [
    BranchAttention {
        code: AttentionCode::LocalOnlyOld,
        when: "a local-only branch older than localOnlyReport",
        check: |s, state| {
            let old = s.now.duration_since(s.branch.tip.time);
            (state == BranchState::LocalOnly && old > s.settings.local_only_report).then(|| {
                format!(
                    "local-only, its tip {} old, past the {} allowed",
                    age(old),
                    age(s.settings.local_only_report)
                )
            })
        },
    },
    BranchAttention {
        code: AttentionCode::Diverged,
        when: "a branch in the diverged state",
        check: |s, state| (state == BranchState::Diverged).then(|| upstream_detail(s)),
    },
    BranchAttention {
        code: AttentionCode::UpstreamGone,
        when: "a branch in the upstream-gone state",
        check: |s, state| (state == BranchState::UpstreamGone).then(|| upstream_detail(s)),
    },
    BranchAttention {
        code: AttentionCode::ArchiveFailed,
        when: "a branch whose archive failed its compare",
        check: |s, _| {
            s.repository
                .backup
                .archives
                .iter()
                .find(|a| {
                    a.branch == s.branch.name
                        && a.oid == s.branch.tip.oid
                        && a.compared == Some(false)
                })
                .map(|a| format!("{} failed its compare", a.reference))
        },
    },
];

/// When a lock was taken: when its file was written, else the time its
/// reason names.
fn locked_since(facts: &LockFacts) -> Option<jiff::Timestamp> {
    facts.written.or(facts.lock.since)
}

fn upstream_detail(s: &BranchSubject<'_>) -> String {
    s.branch.upstream.as_ref().map_or(String::new(), |u| {
        format!(
            "{}: {} ahead, {} behind{}",
            u.reference,
            u.ahead,
            u.behind,
            if u.gone { ", gone" } else { "" }
        )
    })
}

/// Every attention code the worktree's facts and test results call for.
pub fn worktree(subject: &WorktreeSubject<'_>, tests: &[TestResult]) -> Vec<Note<AttentionCode>> {
    WORKTREE_ATTENTION
        .iter()
        .filter_map(|a| (a.check)(subject, tests).map(|detail| Note::new(a.code, detail)))
        .collect()
}

/// Every attention code the branch's facts and state call for.
pub fn branch(subject: &BranchSubject<'_>, state: BranchState) -> Vec<Note<AttentionCode>> {
    BRANCH_ATTENTION
        .iter()
        .filter_map(|a| (a.check)(subject, state).map(|detail| Note::new(a.code, detail)))
        .collect()
}
