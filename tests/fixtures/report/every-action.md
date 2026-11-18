# `sweep` report

Started 2026-11-19T09:12:00Z. The `rules` command defines each rule id and code named here.

**Totals:** `archives.failed` 1, `archives.verified` 2, `archives.written` 3, `attention` 7, `branches.delete` 2, `branches.keep` 11, `failed` 2, `problems` 1, `repositories` 4, `rescues` 1, `worktrees.keep` 9, `worktrees.prune` 1, `worktrees.remove` 1.

## Attention

What a person must settle, the most urgent first.

| Attention | Repository | Item | Rule | Reasons | Survives |
| --- | --- | --- | --- | --- | --- |
| `no-default-branch`: no refs/remotes/origin/HEAD, manifest entry or --default-branch | `/r/nodefault` | repository | — | — | — |
| `archive-failed`: archive refs/archive/preview/spike/2026-11-19T09:12:00Z failed its compare: rev-list: missing object | `/r/app` | branch `preview/spike` | `branch.disposable` | `archive-failed` | **on no remote** |
| `broken`: its .git file names no worktree of this repository | `/r/app` | worktree `/r/app.worktrees/odd` | `worktree.keep` | `broken` | **on no remote** |
| `uncommitted`: 2 uncommitted paths | `/r/app` | worktree `/r/app.worktrees/wip` | `worktree.keep` | `not-landed`: HEAD is not merged into refs/remotes/origin/main<br/>`uncommitted`<br/>`not-idle`: idle 5m of the 1h needed | **on no remote** |
| `locked-landed`: landed and locked 4d, longer than 1d | `/r/app` | worktree `/r/app.worktrees/old` | `worktree.keep` | `locked`: locked by nhooey since 2026-11-15T09:00:00Z | merged `refs/remotes/origin/main` |
| `diverged`: ahead 1, behind 2 of refs/remotes/origin/drifted | `/r/app` | branch `drifted` | `branch.diverged` | `diverged` | **on no remote** |
| `upstream-gone`: refs/remotes/origin/orphan is gone | `/r/app` | branch `orphan` | `branch.upstream-gone` | `upstream-gone` | **on no remote** |
| `local-only-old`: tip 9d old, older than 7d | `/r/app` | branch `idea` | `branch.local-only` | `local-only`: no upstream | **on no remote** |

## Changes

The riskiest first.

| Action | Outcome | Repository | Item | Rule | Detail |
| --- | --- | --- | --- | --- | --- |
| remove | failed | `/r/app` | worktree `/r/app.worktrees/stuck` | `worktree.remove` | error: git worktree remove exited 128: busy<br/>merged `refs/remotes/origin/main` |
| keep | failed | `/r/failing` | branch `main` | `branch.default` | error: cannot lock ref refs/heads/main<br/>`default-branch`: the default branch |
| delete | done | `/r/app` | branch `preview/demo` | `branch.disposable` | archive `refs/archive/preview/demo/2026-11-19T09:12:00Z` verified |
| remove | done | `/r/app` | worktree `/r/app.worktrees/fix` | `worktree.remove` | merged `refs/remotes/origin/main`<br/>2048 bytes |
| delete | done | `/r/app` | branch `fix` | `branch.landed` | merged `refs/remotes/origin/main` |
| delete | not attempted | `/r/app` | branch `skipped` | `branch.landed` | merged `refs/remotes/origin/main` |
| rescue | done | `/r/app` | branch `lost` | — | tip `5555555555555555555555555555555555555555`<br/>archive `refs/archive/lost/2026-11-19T09:12:00Z` verified |
| prune | done | `/r/app` | worktree `/r/app.worktrees/gone` | `worktree.prune` | — |

## Kept

By rule id, then reason code.

| Rule | Reasons | Repository | Item | Survives |
| --- | --- | --- | --- | --- |
| `branch.landed` | `checked-out`: checked out in /r/app.worktrees/stuck, which stays | `/r/app` | branch `stuck` | merged `refs/remotes/origin/main` |
| `branch.landed` | `checked-out`: checked out in /r/app.worktrees/old | `/r/app` | branch `old` | merged `refs/remotes/origin/main` |
| `branch.in-review` | `in-review`: refs/remotes/origin/review exists<br/>`checked-out`: checked out in /r/app.worktrees/draft | `/r/app` | branch `review` | pushed `refs/remotes/origin/review` |
| `worktree.keep` | `not-landed`: HEAD is not merged into refs/remotes/origin/main | `/r/app` | worktree `/r/app.worktrees/draft` | pushed `refs/remotes/origin/review` |
| `worktree.keep` | `archive-failed`: branch preview/spike stays: archive refs/archive/preview/spike/2026-11-19T09:12:00Z failed its compare: rev-list: missing object | `/r/app` | worktree `/r/app.worktrees/spike` | **on no remote** |

Not listed: 7 kept only for `main-worktree`, `no-default-branch`, `default-branch` or `checked-out`.

Nothing to do in `/r/quiet`.

## Compare pass

Each archive is compared in the local backup before its branch is deleted.

| Repository | Backup | Planned | Archived | Verified | Failed |
| --- | --- | --- | --- | --- | --- |
| `/r/app` | `/h/backups/git-archive/n/app.git` | 0 | 3 | 2 | 1 |

| Repository | Failed archive | Tip | Detail |
| --- | --- | --- | --- |
| `/r/app` | `refs/archive/preview/spike/2026-11-19T09:12:00Z` | `eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee` | rev-list: missing object |

3 new archives in \~/backups/git-archive/; back up \~/backups/
