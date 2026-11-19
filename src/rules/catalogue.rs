//! What `rules` prints: every rule, worktree test, reason code and
//! attention code, each with its text, read from the tables the plan runs.

use serde::Serialize;

use crate::model::ids::{
    AttentionCode, BranchState, CommandName, ReasonCode, RuleId, TestName, WorktreeAction,
};
use crate::model::report::SCHEMA;
use crate::rules::attention::{BRANCH_ATTENTION, WORKTREE_ATTENTION};
use crate::rules::branch::BRANCH_RULES;
use crate::rules::worktree::{WORKTREE_RULES, WORKTREE_TESTS};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Catalogue {
    pub schema: u32,
    pub command: CommandName,
    /// In the order they are tried; the first a branch matches decides.
    pub branch_rules: Vec<BranchRuleEntry>,
    /// In the order they run.
    pub worktree_tests: Vec<WorktreeTestEntry>,
    /// In the order they are tried.
    pub worktree_rules: Vec<WorktreeRuleEntry>,
    pub reasons: Vec<ReasonEntry>,
    pub attention: Vec<AttentionEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BranchRuleEntry {
    pub id: RuleId,
    pub state: BranchState,
    pub when: &'static str,
    pub action: &'static str,
    /// The reason a branch it keeps is given.
    pub reason: Option<ReasonCode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorktreeTestEntry {
    pub name: TestName,
    pub passes: &'static str,
    /// The reason a failure gives.
    pub reason: ReasonCode,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorktreeRuleEntry {
    pub id: RuleId,
    pub when: &'static str,
    pub action: WorktreeAction,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReasonEntry {
    pub code: ReasonCode,
    pub text: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AttentionEntry {
    pub code: AttentionCode,
    /// `worktree` or `branch`.
    pub on: &'static str,
    pub when: &'static str,
}

/// What each reason means, wherever it is given.
pub fn reason_text(code: ReasonCode) -> &'static str {
    match code {
        ReasonCode::FolderMissing => "the worktree's folder is gone",
        ReasonCode::Broken => "the worktree's folder isn't a working worktree of this repository",
        ReasonCode::MainWorktree => "the main worktree, which is never removed",
        ReasonCode::NoDefaultBranch => {
            "the repository's default branch is unknown, so nothing in it is removed"
        }
        ReasonCode::Locked => "the worktree is locked, and a lock is never broken",
        ReasonCode::NotLanded => {
            "the worktree's HEAD isn't merged, and its branch is neither disposable nor content-landed"
        }
        ReasonCode::Uncommitted => "the worktree has uncommitted changes, tracked or untracked",
        ReasonCode::KeepPatternFiles => "the worktree holds kept files of its own",
        ReasonCode::NotIdle => "the worktree's HEAD, logs/HEAD or index changed within `idle`",
        ReasonCode::DefaultBranch => "the default branch",
        ReasonCode::UpstreamGone => "the branch's upstream is configured and gone",
        ReasonCode::Diverged => "the branch is both ahead of and behind its upstream",
        ReasonCode::InReview => "the branch's upstream exists",
        ReasonCode::LocalOnly => "the branch has no upstream",
        ReasonCode::CheckedOut => "the branch is checked out in a worktree that stays",
        ReasonCode::ArchiveFailed => {
            "the branch's archive failed its compare, so it stays with its worktree"
        }
    }
}

/// The catalogue, from the rules' own tables.
pub fn catalogue() -> Catalogue {
    Catalogue {
        schema: SCHEMA,
        command: CommandName::Rules,
        branch_rules: BRANCH_RULES
            .iter()
            .map(|rule| BranchRuleEntry {
                id: rule.id,
                state: rule.state,
                when: rule.when,
                action: rule.disposal.text(),
                reason: rule.disposal.reason(),
            })
            .collect(),
        worktree_tests: WORKTREE_TESTS
            .iter()
            .map(|test| WorktreeTestEntry {
                name: test.name,
                passes: test.passes,
                reason: test.reason,
            })
            .collect(),
        worktree_rules: WORKTREE_RULES
            .iter()
            .map(|rule| WorktreeRuleEntry {
                id: rule.id,
                when: rule.when,
                action: rule.action,
            })
            .collect(),
        reasons: ReasonCode::ALL
            .iter()
            .map(|&code| ReasonEntry {
                code,
                text: reason_text(code),
            })
            .collect(),
        attention: WORKTREE_ATTENTION
            .iter()
            .map(|a| AttentionEntry {
                code: a.code,
                on: "worktree",
                when: a.when,
            })
            .chain(BRANCH_ATTENTION.iter().map(|a| AttentionEntry {
                code: a.code,
                on: "branch",
                when: a.when,
            }))
            .collect(),
    }
}

/// The catalogue for a person at a terminal.
pub fn text(catalogue: &Catalogue) -> String {
    let section = |title: &str, lines: Vec<String>| format!("{title}\n{}\n", lines.join("\n"));
    let numbered = |n: usize, line: String| format!("  {}. {line}", n + 1);
    let sections = [
        section(
            "Branches: the first rule a branch matches gives its state and action.",
            catalogue
                .branch_rules
                .iter()
                .enumerate()
                .map(|(n, rule)| {
                    let reason = rule
                        .reason
                        .map_or(String::new(), |code| format!(" ({code})"));
                    numbered(
                        n,
                        format!(
                            "{}, {}: {}. {}{reason}.",
                            rule.id,
                            rule.state,
                            rule.when,
                            capitalised(rule.action)
                        ),
                    )
                })
                .collect(),
        ),
        section(
            "Worktree tests, in order; each failed test is a reason.",
            catalogue
                .worktree_tests
                .iter()
                .enumerate()
                .map(|(n, test)| {
                    numbered(
                        n,
                        format!("{}: {}. Else {}.", test.name, test.passes, test.reason),
                    )
                })
                .collect(),
        ),
        section(
            "Worktree rules: the first that applies gives the action.",
            catalogue
                .worktree_rules
                .iter()
                .enumerate()
                .map(|(n, rule)| {
                    numbered(
                        n,
                        format!(
                            "{}: {}. {}.",
                            rule.id,
                            rule.when,
                            capitalised(rule.action.as_str())
                        ),
                    )
                })
                .collect(),
        ),
        section(
            "Reasons:",
            catalogue
                .reasons
                .iter()
                .map(|r| format!("  {}: {}.", r.code, r.text))
                .collect(),
        ),
        section(
            "Attention:",
            catalogue
                .attention
                .iter()
                .map(|a| format!("  {}, on a {}: {}.", a.code, a.on, a.when))
                .collect(),
        ),
    ];
    sections.join("\n")
}

fn capitalised(text: &str) -> String {
    match text.split_at_checked(1) {
        Some((first, rest)) => format!("{}{rest}", first.to_uppercase()),
        None => text.to_owned(),
    }
}
