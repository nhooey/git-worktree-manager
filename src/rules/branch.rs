//! The branch rules: the first a branch matches gives its state and
//! action.

use jiff::Timestamp;

use crate::model::Note;
use crate::model::facts::{BranchFacts, RepositoryFacts};
use crate::model::ids::{BranchAction, BranchState, ReasonCode, RuleId, WorktreeAction};
use crate::model::plan::{BranchPlan, WorktreePlan};
use crate::model::report::Upstream;
use crate::model::settings::Settings;
use crate::rules::{age, attention, survival};

/// One branch, with what its rules read besides it.
#[derive(Clone, Copy, Debug)]
pub struct BranchSubject<'a> {
    pub branch: &'a BranchFacts,
    pub repository: &'a RepositoryFacts,
    pub settings: &'a Settings,
    pub now: Timestamp,
}

/// What a rule does with the branches it matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposal {
    /// Kept, with this reason, whatever `finish --abandon` says.
    Keep(ReasonCode),
    /// Kept, with this reason, until it lands or `finish --abandon`
    /// archives it, then deletes it.
    KeepUntilLanded(ReasonCode),
    Delete,
    /// Archived, unless its tip is merged or archived already, then deleted.
    ArchiveThenDelete,
}

impl Disposal {
    pub fn text(self) -> &'static str {
        match self {
            Self::Keep(_) => "keep",
            Self::KeepUntilLanded(_) => "keep; finish --abandon archives, then deletes",
            Self::Delete => "delete",
            Self::ArchiveThenDelete => "archive, then delete",
        }
    }

    /// The reason a kept branch is given.
    pub fn reason(self) -> Option<ReasonCode> {
        match self {
            Self::Keep(code) | Self::KeepUntilLanded(code) => Some(code),
            Self::Delete | Self::ArchiveThenDelete => None,
        }
    }
}

pub struct BranchRule {
    pub id: RuleId,
    pub state: BranchState,
    /// When it matches, in the node's words.
    pub when: &'static str,
    pub disposal: Disposal,
    pub matches: fn(&BranchSubject<'_>) -> bool,
    /// What matched, with its values: a kept branch's reason.
    pub detail: fn(&BranchSubject<'_>) -> String,
}

/// The branch rules, in the order they are tried. The last matches every
/// branch.
pub static BRANCH_RULES: [BranchRule; 9] = [
    BranchRule {
        id: RuleId::BranchDefault,
        state: BranchState::DefaultBranch,
        when: "it is the default branch",
        disposal: Disposal::Keep(ReasonCode::DefaultBranch),
        matches: |s| {
            s.repository
                .default
                .as_ref()
                .is_some_and(|d| d.branch == s.branch.name)
        },
        detail: |s| format!("{} is the default branch", s.branch.name),
    },
    BranchRule {
        id: RuleId::BranchNoDefault,
        state: BranchState::NoDefaultBranch,
        when: "the repository's default branch is unknown",
        disposal: Disposal::Keep(ReasonCode::NoDefaultBranch),
        matches: |s| s.repository.default.is_none(),
        detail: |_| "no default branch is known, so nothing is removed".to_owned(),
    },
    BranchRule {
        id: RuleId::BranchLanded,
        state: BranchState::Landed,
        when: "its tip is merged",
        disposal: Disposal::Delete,
        matches: |s| s.branch.tip.merged == Some(true),
        detail: |s| format!("its tip is merged into {}", default_ref(s)),
    },
    BranchRule {
        id: RuleId::BranchDisposable,
        state: BranchState::Disposable,
        when: "its name matches a disposable pattern",
        disposal: Disposal::ArchiveThenDelete,
        matches: |s| disposable_pattern(s).is_some(),
        detail: |s| {
            format!(
                "it matches the disposable pattern {}",
                disposable_pattern(s).unwrap_or_default()
            )
        },
    },
    BranchRule {
        id: RuleId::BranchContentLanded,
        state: BranchState::ContentLanded,
        when: "its content has landed",
        disposal: Disposal::ArchiveThenDelete,
        matches: |s| s.branch.content_landed == Some(true),
        detail: |s| format!("{} holds every change it made", default_ref(s)),
    },
    BranchRule {
        id: RuleId::BranchUpstreamGone,
        state: BranchState::UpstreamGone,
        when: "its upstream is configured and gone",
        disposal: Disposal::KeepUntilLanded(ReasonCode::UpstreamGone),
        matches: |s| s.branch.upstream.as_ref().is_some_and(|u| u.gone),
        detail: |s| format!("its upstream {} is gone", upstream_ref(s)),
    },
    BranchRule {
        id: RuleId::BranchDiverged,
        state: BranchState::Diverged,
        when: "it is both ahead of and behind its upstream",
        disposal: Disposal::KeepUntilLanded(ReasonCode::Diverged),
        matches: |s| {
            s.branch
                .upstream
                .as_ref()
                .is_some_and(|u| u.ahead > 0 && u.behind > 0)
        },
        detail: |s| {
            let (ahead, behind) = s
                .branch
                .upstream
                .as_ref()
                .map_or((0, 0), |u| (u.ahead, u.behind));
            format!(
                "it is {ahead} ahead of and {behind} behind {}",
                upstream_ref(s)
            )
        },
    },
    BranchRule {
        id: RuleId::BranchInReview,
        state: BranchState::InReview,
        when: "its upstream exists",
        disposal: Disposal::KeepUntilLanded(ReasonCode::InReview),
        matches: |s| s.branch.upstream.as_ref().is_some_and(|u| !u.gone),
        detail: |s| format!("its upstream {} exists", upstream_ref(s)),
    },
    BranchRule {
        id: RuleId::BranchLocalOnly,
        state: BranchState::LocalOnly,
        when: "otherwise",
        disposal: Disposal::KeepUntilLanded(ReasonCode::LocalOnly),
        matches: |_| true,
        detail: |s| {
            format!(
                "it has no upstream, and its tip is {} old",
                age(s.now.duration_since(s.branch.tip.time))
            )
        },
    },
];

fn default_ref<'a>(subject: &BranchSubject<'a>) -> &'a str {
    subject
        .repository
        .default
        .as_ref()
        .map_or("the default branch", |d| d.reference.as_str())
}

fn upstream_ref<'a>(subject: &BranchSubject<'a>) -> &'a str {
    subject
        .branch
        .upstream
        .as_ref()
        .map_or("", |u| u.reference.as_str())
}

/// The first `disposable` pattern the branch's name matches, in git's
/// glob syntax, where `*` matches `/` as `git branch --list` has it.
fn disposable_pattern<'a>(subject: &BranchSubject<'a>) -> Option<&'a str> {
    subject
        .settings
        .disposable
        .iter()
        .map(String::as_str)
        .find(|pattern| {
            gix::glob::wildmatch(
                (*pattern).into(),
                subject.branch.name.as_str().into(),
                gix::glob::wildmatch::Mode::empty(),
            )
        })
}

/// The first rule the branch matches.
pub fn rule(subject: &BranchSubject<'_>) -> &'static BranchRule {
    BRANCH_RULES
        .iter()
        .find(|rule| (rule.matches)(subject))
        .unwrap_or(&BRANCH_RULES[BRANCH_RULES.len() - 1])
}

/// The branch's plan under `rule`, given what the worktree rules decided:
/// a branch checked out in a worktree that stays is kept. `abandon` is
/// `finish --abandon` on the worktree it is checked out in. The archive
/// ref is named later, once every branch that needs one is known.
pub fn plan(
    subject: &BranchSubject<'_>,
    rule: &BranchRule,
    worktrees: &[WorktreePlan],
    abandon: bool,
) -> BranchPlan {
    let branch = subject.branch;
    let checked_out: Vec<&WorktreePlan> = worktrees
        .iter()
        .filter(|w| w.branch.as_deref() == Some(branch.name.as_str()))
        .collect();
    let holder = checked_out
        .iter()
        .find(|w| w.action == WorktreeAction::Keep);
    let disposal = match rule.disposal {
        Disposal::KeepUntilLanded(_) if abandon => Disposal::ArchiveThenDelete,
        disposal => disposal,
    };
    let action = match (disposal.reason(), holder) {
        (None, None) => BranchAction::Delete,
        _ => BranchAction::Keep,
    };
    let held = holder.map(|w| {
        Note::new(
            ReasonCode::CheckedOut,
            format!("checked out in {}, which stays", w.path.display()),
        )
    });
    let kept = disposal
        .reason()
        .map(|code| Note::new(code, (rule.detail)(subject)));
    BranchPlan {
        name: branch.name.clone(),
        tip: branch.tip.oid,
        upstream: branch.upstream.as_ref().map(|u| Upstream {
            reference: u.reference.clone(),
            gone: u.gone,
            ahead: u.ahead,
            behind: u.behind,
        }),
        state: rule.state,
        action,
        rule: rule.id,
        archive: None,
        worktree: holder.or(checked_out.first()).map(|w| w.path.clone()),
        reasons: held.into_iter().chain(kept).collect(),
        survives: survival::survives(&branch.tip, subject.repository),
        attention: attention::branch(subject, rule.state),
    }
}
