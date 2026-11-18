# nix-worktrees

Removes git worktrees and branches once their work has landed, and loses nothing on the way. It also keeps worktrees' build output outside them and shared.

It holds one program, `worktrees`. Git hooks, dev shells, agent sessions and people all call it the same way, and it runs without Nix.

## Install and run

- **With Nix:** `nix run github:nhooey/nix-worktrees -- status`, or the `worktrees` package (`packages.<system>.default`, also `.worktrees`).
- **Without Nix:** `bin/worktrees` from a clone, or `python3 -m worktrees` with the clone on `PYTHONPATH`. It needs Python 3.9 or later and nothing outside the standard library. `pip install .` gives the Python commands only, without the hook entry point.
- **It runs** `git` 2.31 or later (2.36 for the hook), `ps` and `awk`.

## The commands

Every command takes `--help` and long options only. `sweep`, `status`, `verdict`, `log` and `archive` take `--json`.

| Command | Does |
| --- | --- |
| `worktrees sweep [--dry-run] [--json] [--all <root>]... [--default-branch <branch>\|<checkout>=<branch>]... [--fetch] [<checkout>...]` | the sweep below, in each repository; `--dry-run` changes nothing and says what it would do |
| `worktrees status [--json] [--all <root>]... [--default-branch ...]... [<checkout>...]` | one row per worktree and per local branch: the verdict the sweep would reach, every reason it keeps one, and where its data survives |
| `worktrees verdict <worktree-or-branch> [--checkout <checkout>] [--default-branch ...]` | one verdict with every test's result |
| `worktrees log [--branch <name>] [--since <time>] [--event <kind>]... [--checkout <checkout>]` | queries the repository's event log |
| `worktrees archive <branch> [--dry-run] [--checkout <checkout>]` | pushes the branch's tip to the local backup as `refs/archive/<branch>/<time>`, and compares it there |
| `worktrees env`, `provision`, `clean`, `sessions` | build environment, provisioning, cleaning a worktree's ignored build output, and a checkout's Claude Code sessions |

- **Repositories:** each `<checkout>` names one, and `--all <root>` adds every main checkout one level under `<root>`. Two checkouts of one repository are swept once.
- **The default branch** is the target of `refs/remotes/origin/HEAD`. `--default-branch master` sets it for every checkout, and `--default-branch ~/src/app=master` for one. A checkout with neither gets the verdict "no default branch, nothing removed", and the other checkouts go on.
- **Exit status:** 0 done; 1 a repository failed, or an archive failed its compare; 2 bad usage; 3 `clean` refused.
- **The last line** of a sweep that archives names the new archives and asks for a backup: `N new archives in ~/backups/git-archive/; back up ~/backups/`. Nothing backs up `~/backups` but its owner.

### The JSON

`worktrees sweep --dry-run --json ~/src/app` prints:

- **Top level:** `mode` (`dry-run`, `apply` or `report`), `dry_run`, `time`, `repositories`, `totals` and `backup_note`, the last line above or null.
- **Each repository:** `repository`, `common_dir`, `state_dir`, `default_branch`, `default_ref`, `problem`, `settings`, `worktrees`, `branches`, `backfill`, `archives_pruned`, `rescues`, `lock_reports`, `backup` and `totals`. A repository that failed is `{"repository", "error"}`.
- **Each worktree and branch:**
  - `action`: `remove`, `prune`, `clean`, `archive+delete`, `delete` or `keep`;
  - `would: true` under a dry run, else `done` and any `error`;
  - `reasons`: every reason it is kept, as `{code, detail}`;
  - `survives`: where its data is besides here. The entries are `merged into origin/main`, `pushed as origin/<b> at <sha>`, `archive refs/archive/<b>/<time> (would write to the local backup <path>, would compare)`, or `nowhere else: kept`;
  - for a worktree, its `lock` with the reason's `who`, `pid`, `since`, and whether the process is `alive`;
  - for a branch, its `state` and `archive_ref`.
- **The backfill:** `backfill.archives` lists the archive refs it writes, each with its branch, tip and `backup`.
- **The backup:** `backup` has the bare repository's `path`, the counts `archived`, `verified` and `failed`, and `compares`, one per archive: `branch`, `ref`, the source `tip`, the `backup_tip` the ref holds there, each check's result in `checks`, `ok` and any `error`. Under a dry run, `compares` lists what it would compare.

## The sweep

A sweep runs after the default branch moves, started detached by the hook, or by hand. It takes `flock` on `<state>/sweep.lock`; a second sweep that finds it held leaves `sweep.pending`, and the first runs again. Its report goes to `<state>/last-sweep.json`.

In order, in each repository:

1. **Backfill**, once, when the log holds no snapshot:
   - a snapshot of every worktree and branch goes into the log;
   - every branch that hasn't landed is archived to the local backup.
2. **Prune** worktrees whose folder is gone and that aren't locked.
3. **Remove** each worktree that passes every test:
   - not the main checkout;
   - a default branch known;
   - not locked;
   - landed;
   - clean;
   - no kept files unlike the main checkout's;
   - its git directory idle for `worktrees.idle`.
4. **Archive and compare** every archive the sweep writes, before anything is removed: see [The local backup](#the-local-backup).
5. **Clean** the ignored build output of a worktree that hasn't landed but has been idle a day.
6. **Delete branches** by the branch table below. Deletion is `git update-ref -d refs/heads/<b> <tip>`, by the tip the sweep tested.
7. **Prune archives** the default branch now holds: the `refs/archive/*` left in a clone by an older sweep.
8. **Rescue** branches deleted by hand whose tips are no longer reachable and the backup lacks: they are archived from the tip in the log.
9. **Report** landed worktrees locked longer than `worktrees.lockReport`.

### Nothing lost

Nothing is removed unless its data is provably off this clone:

- **merged:** an ancestor of `origin/<default>`;
- **pushed:** a remote-tracking ref at the same tip;
- **archived:** in the local backup, and compared there.

A local ref, a tag or an archive ref in the same clone counts for nothing, since it goes when the clone does. Otherwise the branch is archived first, or kept.

- **Uncommitted changes keep a worktree.**
- **Kept files keep a worktree:** an ignored file a keep pattern matches, unless it holds the same bytes as the main checkout's copy.
- **What `clean` deletes is build output:** ignored files no keep pattern matches.
- **No file backups:** the program copies no worktree files. What can't be shown to be elsewhere blocks removal instead.

| Branch state | Action |
| --- | --- |
| `default-branch` | keep |
| `no-default-branch` | keep |
| `landed`: its tip is an ancestor of the default | delete |
| `preview`: `preview/*` | delete if merged or pushed, else archive, then delete |
| `content-landed`: merging it into the default changes nothing | delete if merged or pushed, else archive, then delete |
| `upstream-gone`, `diverged`, `in-review`, `local-only` | keep, archived once by the backfill |

A branch checked out in a worktree that stays is kept.

**Content landed** is tested read-only. The changes from the merge base to the branch's tip (`git diff-tree -r -z --no-renames`) are compared with the default's tree (`git ls-tree -r -z --full-tree`). `git merge-tree --write-tree` would answer too, but it writes objects, and it refreshes the mtimes of existing ones even through `GIT_ALTERNATE_OBJECT_DIRECTORIES`. A dry run changes nothing.

### The local backup

Archives go to one bare repository per project, `~/backups/git-archive/<owner>/<repo>.git`, made with `git init --bare` the first time. The owner and repository come from `origin`'s URL, or are `local/<folder>` for a clone with no `origin`. Nothing goes to `origin`, so a branch that was never pushed stays private, even in a public fork.

- **Archiving:** one `git push --no-verify <bare> <tip>:refs/archive/<branch>/<time>...` per sweep, which copies only the objects the backup lacks.
- **The compare pass:** git checks each archive in the backup before anything is deleted. `git rev-parse <ref>` must equal the branch's tip, `git rev-list --objects --missing=error <tip>` must pass, and `git fsck --connectivity-only`, once a sweep, must pass. A branch whose archive fails any check stays, with its worktree, and the sweep exits 1.
- **READMEs:** `~/backups/git-archive/README.md` and one per owner folder say what is there and how to restore, and `~/backups/README.md` gets a line pointing to them. A dry run writes none of them.
- **Restoring:** from a clone of the project, `git fetch ~/backups/git-archive/<owner>/<repo>.git 'refs/archive/*:refs/archive/*'`, then `git branch <branch> refs/archive/<branch>/<time>`.

### Locks

A lock's reason is `<who> since <ISO time>`, or `claude agent agent-<id> (pid <pid> start <time>)`. The second form is checked against `ps -o lstart= -p <pid>`. The process is alive when that pid started at that time.

A lock keeps its worktree either way; the report says whose it is and whether its process is alive.

### The dry run

`--dry-run` is read-only:

- every git command runs with `--no-optional-locks`;
- ignored files are listed with `git ls-files --others --ignored --exclude-standard --directory`, without walking build trees;
- no lock is taken and nothing is logged.

The tests check that refs, `git worktree list --porcelain`, every file's mtime in the repository, and the state directory are unchanged.

## Settings

Settings come from git config, so each repository can set its own. Defaults are in `worktrees/defaults.json`.

| Key | Default | Means |
| --- | --- | --- |
| `worktrees.sweep` | `report` | `apply` lets a sweep the hook starts act; `report` only writes its report |
| `worktrees.idle` | `1h` | how long a worktree's `HEAD`, `logs/HEAD` and index must be unchanged before removal |
| `worktrees.lockReport` | `1d` | how long a landed worktree may stay locked before the report names it |

## The log and state

Each repository has `${XDG_STATE_HOME:-~/.local/state}/worktrees/<repo>-<hash8>/`, holding:

- **`events.jsonl`:** every ref update the hook saw, and every snapshot, archive, removal and deletion the sweep made. It is append-only, and each line is a single `write` under 4 KB.
- **`sweep.lock` and `sweep.pending`:** the sweep's lock, and the note that another sweep is due.
- **`last-sweep.json`:** the last sweep's report.

## The hook

`worktrees hook-ref-update` is a `sh` function in `bin/worktrees`, for git's `reference-transaction` hook. It makes no git call and starts no Python. On `committed` it does three things:
- one awk process appends a line per ref to the log;
- awk reads the clock through `srand()`;
- when `$WORKTREES_DEFAULT_REF` is among the refs, it starts `worktrees sweep --from-hook`, detached.

The shim installed in `.git/hooks/reference-transaction` is:

```sh
#!/bin/sh
[ "$1" = committed ] || exit 0
w=/nix/store/…-worktrees/bin/worktrees
[ -x "$w" ] || exit 0
WORKTREES_LOG=…/events.jsonl \
WORKTREES_DEFAULT_REF=refs/remotes/origin/main \
exec "$w" hook-ref-update "$@"
```

What the hook costs:

- **Three starts per transaction:** git starts the hook for `preparing`, `prepared` and `committed`. On a Mac where a process start takes about 5.5 ms, that alone costs about 15 ms, so the shim leaves at once for any state but `committed`.
- **A fetch is one transaction,** fast-forwards and forced updates alike. This holds for git 2.56 and for Apple's git 2.54. On a 1,000-ref fetch the hook adds about 4 ms over an empty hook. The test is `worktrees/tests/test_hooks.py`.
- **Ref packing is one transaction per ref:** git's auto maintenance packs refs after a large fetch, and deletes each loose ref in a transaction of its own. With 1,000 loose refs that is 1,000 hook runs, about 28 s against 0.9 s. `maintenance.auto=false` avoids it.
- **Deletions have no old value:** git 2.56 passes an all-zero old value for every branch deletion, `git branch -d` included. So a rescue takes the deleted tip from the last value the log holds for that ref.
- **No reflog action:** git sets no `GIT_REFLOG_ACTION` for `git commit`, nor for a fetch's ref updates. A line has `action` only when git sets it.

## The build root

`worktrees env` prints export lines that put each worktree's build output under `${XDG_CACHE_HOME:-~/.cache}/worktree-builds/<repo>-<hash8>/`, outside the worktree. Caches that can be shared go under `shared/`; per-worktree output goes under `wt/<wt-id>/`. Run it as `eval "$(worktrees env)"`. It detects the languages from the files at the worktree's root, or takes them from `--languages`.

## Development

`nix develop` enters the dev shell, with `worktrees` and `tests` as commands. `nix flake check` runs:
- treefmt with nixfmt, ruff and shellcheck;
- the package build, and a run of the package;
- every test.

Without Nix, run `python3 -m unittest discover --start-directory worktrees/tests --top-level-directory .`.
