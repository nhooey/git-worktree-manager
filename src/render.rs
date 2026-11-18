//! Text and Markdown rendered from a report, by pure functions; no caller
//! parses either. Both print paths as the report holds them.

mod markdown;
mod text;

use crate::model::Note;
use crate::model::ids::{
    AttentionCode, BranchAction, OutcomeKind, ReasonCode, RuleId, WorktreeAction,
};
use crate::model::report::{
    ArchiveReport, BranchReport, Report, RepositoryReport, Survival, WorktreeReport,
};

/// The report for a person at a terminal: `--format text`.
pub fn text(report: &Report) -> String {
    text::render(report)
}

/// The report as Markdown, for `status` and `sweep`: `--format markdown`.
pub fn markdown(report: &Report) -> String {
    markdown::render(report)
}

/// A worktree's action or a branch's, which says which it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Worktree(WorktreeAction),
    Branch(BranchAction),
}

impl Action {
    fn kind(self) -> &'static str {
        match self {
            Self::Worktree(_) => "worktree",
            Self::Branch(_) => "branch",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Worktree(a) => a.as_str(),
            Self::Branch(a) => a.as_str(),
        }
    }
}

/// A worktree or a branch, as both renderers show it.
struct Item<'a> {
    /// A worktree's path, or a branch's name.
    name: String,
    action: Action,
    /// The action is `keep`.
    kept: bool,
    outcome: OutcomeKind,
    error: Option<&'a str>,
    rule: RuleId,
    reasons: &'a [Note<ReasonCode>],
    attention: &'a [Note<AttentionCode>],
    archive: Option<&'a ArchiveReport>,
    survives: &'a [Survival],
    bytes: Option<u64>,
}

impl Item<'_> {
    /// An action the run takes, or one that failed, whatever the action.
    fn changes(&self) -> bool {
        !self.kept || self.outcome == OutcomeKind::Failed
    }

    /// Kept for reasons no person need read: not when it changes, needs
    /// attention, or stayed though its rule would have acted.
    fn quiet(&self) -> bool {
        !self.changes()
            && self.attention.is_empty()
            && !acts(self.rule)
            && !self.reasons.is_empty()
            && self.reasons.iter().all(|r| routine(r.code))
    }

    /// The outcome in words. An action the run neither did nor failed,
    /// outside a dry run, was not attempted.
    fn outcome(&self) -> &'static str {
        match self.outcome {
            OutcomeKind::None if !self.kept => "not attempted",
            outcome => outcome.as_str(),
        }
    }
}

/// A rule that removes, prunes or deletes what it decides.
fn acts(rule: RuleId) -> bool {
    matches!(
        rule,
        RuleId::BranchLanded
            | RuleId::BranchDisposable
            | RuleId::BranchContentLanded
            | RuleId::WorktreeRemove
            | RuleId::WorktreePrune
    )
}

/// A reason that says only what the report says elsewhere: the main
/// worktree, the default branch, a branch checked out in a worktree that
/// stays, whose own row says why, and everything in a repository with no
/// default branch, which its problem says.
fn routine(code: ReasonCode) -> bool {
    matches!(
        code,
        ReasonCode::MainWorktree
            | ReasonCode::DefaultBranch
            | ReasonCode::CheckedOut
            | ReasonCode::NoDefaultBranch
    )
}

fn worktree(w: &WorktreeReport) -> Item<'_> {
    Item {
        name: w.path.display().to_string(),
        action: Action::Worktree(w.action),
        kept: w.action == WorktreeAction::Keep,
        outcome: w.outcome,
        error: w.error.as_deref(),
        rule: w.rule,
        reasons: &w.reasons,
        attention: &w.attention,
        archive: w.archive.as_ref(),
        survives: &w.survives,
        bytes: w.bytes,
    }
}

fn branch(b: &BranchReport) -> Item<'_> {
    Item {
        name: b.name.clone(),
        action: Action::Branch(b.action),
        kept: b.action == BranchAction::Keep,
        outcome: b.outcome,
        error: b.error.as_deref(),
        rule: b.rule,
        reasons: &b.reasons,
        attention: &b.attention,
        archive: b.archive.as_ref(),
        survives: &b.survives,
        bytes: None,
    }
}

/// A repository's worktrees, then its branches.
fn items(r: &RepositoryReport) -> impl Iterator<Item = Item<'_>> {
    r.worktrees
        .iter()
        .map(worktree)
        .chain(r.branches.iter().map(branch))
}

/// The details of `notes`, so a note whose detail repeats one of them can
/// show its code alone.
fn details<C>(notes: &[Note<C>]) -> Vec<&str> {
    notes.iter().map(|n| n.detail.as_str()).collect()
}

/// The totals that aren't zero, but `skip`, as `name count` pairs.
fn counts<'a>(
    totals: &'a crate::model::report::Totals,
    skip: &'a [&str],
) -> impl Iterator<Item = (&'a str, u64)> {
    totals
        .iter()
        .filter(move |(key, n)| **n > 0 && !skip.contains(&key.as_str()))
        .map(|(key, n)| (key.as_str(), *n))
}

/// The format characters, Unicode's category Cf as of Unicode 15.1, and
/// the line and paragraph separators, Zl and Zp: invisible, they can
/// reorder or hide what a line shows. The standard library has no
/// category table.
const INVISIBLE: [(char, char); 21] = [
    ('\u{00AD}', '\u{00AD}'),
    ('\u{0600}', '\u{0605}'),
    ('\u{061C}', '\u{061C}'),
    ('\u{06DD}', '\u{06DD}'),
    ('\u{070F}', '\u{070F}'),
    ('\u{0890}', '\u{0891}'),
    ('\u{08E2}', '\u{08E2}'),
    ('\u{180E}', '\u{180E}'),
    ('\u{200B}', '\u{200F}'),
    ('\u{2028}', '\u{202E}'),
    ('\u{2060}', '\u{2064}'),
    ('\u{2066}', '\u{206F}'),
    ('\u{FEFF}', '\u{FEFF}'),
    ('\u{FFF9}', '\u{FFFB}'),
    ('\u{110BD}', '\u{110BD}'),
    ('\u{110CD}', '\u{110CD}'),
    ('\u{13430}', '\u{1343F}'),
    ('\u{1BCA0}', '\u{1BCA3}'),
    ('\u{1D173}', '\u{1D17A}'),
    ('\u{E0001}', '\u{E0001}'),
    ('\u{E0020}', '\u{E007F}'),
];

/// `s` with each control, format or separator character written as its
/// Rust escape, so a name can neither drive a terminal, reorder a line
/// nor break one.
fn printable(s: &str) -> String {
    let escaped = |c: char| {
        c.is_control()
            || INVISIBLE
                .iter()
                .any(|&(low, high)| (low..=high).contains(&c))
    };
    s.chars()
        .flat_map(|c| {
            let shown: Vec<char> = if escaped(c) {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            shown
        })
        .collect()
}
