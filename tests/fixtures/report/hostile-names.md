# `sweep` report, dry run

Started 2026-11-19T09:12:00Z. The `rules` command defines each rule id and code named here.

**Totals:** `archives.planned` 1, `attention` 2, `branches.delete` 2, `branches.keep` 6, `repositories` 1, `worktrees.keep` 2, `worktrees.remove` 1.

## Attention

What a person must settle, the most urgent first.

| Attention | Repository | Item | Rule | Reasons | Survives |
| --- | --- | --- | --- | --- | --- |
| `upstream-gone`: gone \\u{1b}\[31mred | `/r/we\|rd_*repo*` | branch `[x](http://e) @someone` | `branch.upstream-gone` | `upstream-gone` | **on no remote** |
| `local-only-old`: tip 9d old\\nsecond line | `/r/we\|rd_*repo*` | branch ```a\|b\\|c``d``` | `branch.local-only` | `local-only`: no upstream \| none | **on no remote** |

## Changes

The riskiest first.

| Action | Outcome | Repository | Item | Rule | Detail |
| --- | --- | --- | --- | --- | --- |
| delete | planned | `/r/we\|rd_*repo*` | branch `*star* <b>&amp;</b>` | `branch.disposable` | archive `refs/archive/*star* <b>&amp;</b>/2026-11-19T09:12:00Z` planned |
| remove | planned | `/r/we\|rd_*repo*` | worktree `` /r/we\|rd_*repo*.worktrees/`tick` `` | `worktree.remove` | merged `refs/remotes/origin/main` |
| delete | planned | `/r/we\|rd_*repo*` | branch `` `tick` `` | `branch.landed` | merged `refs/remotes/origin/main` |

## Kept

By rule id, then reason code.

| Rule | Reasons | Repository | Item | Survives |
| --- | --- | --- | --- | --- |
| `branch.in-review` | `in-review`: refs/remotes/origin/-dash exists | `/r/we\|rd_*repo*` | branch `-dash\u{2028}line\u{2029}para\u{200b}zero` | pushed `refs/remotes/origin/-dash` |
| `branch.in-review` | `in-review`: refs/remotes/origin/\_\_init\_\_ exists | `/r/we\|rd_*repo*` | branch `__init__` | **on no remote** |
| `branch.local-only` | `local-only`: no upstream | `/r/we\|rd_*repo*` | branch `# hash\n- dash` | **on no remote** |
| `worktree.keep` | `not-landed`: HEAD is not merged into refs/remotes/origin/main | `/r/we\|rd_*repo*` | worktree `/r/we\|rd_*repo*.worktrees/-lead\u{1b}[2J\n# hash\u{202e}exe.txt` | **on no remote** |

Not listed: 2 kept only for `main-worktree`, `no-default-branch`, `default-branch` or `checked-out`.

## Compare pass

Each archive is compared in the local backup before its branch is deleted.

| Repository | Backup | Planned | Archived | Verified | Failed |
| --- | --- | --- | --- | --- | --- |
| `/r/we\|rd_*repo*` | `/h/backups/git-archive/local/we\|rd_*repo*.git` | 1 | 0 | 0 | 0 |
