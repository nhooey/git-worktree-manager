# git-worktree-manager

One binary that ends git worktrees and branches once their work has landed, and loses nothing on the way. It also keeps worktrees' build output outside them.

Git runs it as `git worktree-manager` once it is on the `PATH`. Once its hook is installed in a repository (see [Installing the hook](#installing-the-hook)), it records that repository's ref changes and starts a sweep when the default branch moves.

Its jobs:

1. **Plan:** say what happens to every worktree and local branch of a repository, by which rule, and where its commits survive.
2. **Sweep:** carry the plan out.
3. **Finish:** end one worktree now.
4. **Archive:** copy unmerged commits to the local backup, verify them there, and restore them.
5. **Build root:** keep worktrees' build output outside them, and delete one worktree's.
6. **Disk:** say whether there is room for a build.
7. **Hook:** record ref changes, and start the sweep when the default branch moves: see [The hook](#the-hook).

## Installing

- **With Nix:** `nix run github:nhooey/git-worktree-manager -- status`, or the package `packages.<system>.default`, also named `git-worktree-manager`.
- **With Cargo:** `cargo install --path .` from a clone.

Requirements:

- **To build:** Rust, edition 2024.
- **To run:** `git` and `ps`, found on the `PATH`. The package isn't wrapped, so it uses whichever `git` and `ps` the `PATH` gives it: the hook's shim starts the binary once per committed transaction, and a wrapper would add one more process start to each.
- **As a git subcommand:** with `git-worktree-manager` on the `PATH`, `git worktree-manager <command>` runs it, and `git worktree-manager <command> --help` prints that command's help. For the top-level help run `git-worktree-manager --help`: git turns `git worktree-manager --help` into `git help worktree-manager`, which looks for a manual page and finds none.

## The commands

Every command takes `--help`, and long options only.

| Command | Does |
| --- | --- |
| [`status`](#status) | Prints the plan for every worktree and local branch, and changes nothing |
| [`sweep`](#sweep) | Carries the plan out |
| [`finish`](#finish) | Removes one worktree and its branch now, or says why not |
| [`archive write`](#archive) | Archives the branches named, or every unmerged branch whose tip the backup lacks |
| [`archive verify`](#archive) | Runs the compare pass over every archive in the backup |
| [`archive list`](#archive) | Lists the backup's archives |
| [`archive restore`](#archive) | Recreates a branch from its newest archive, or the one at a time |
| [`clean`](#clean) | Deletes a worktree's build output, inside it and under the build root |
| [`env`](#env) | Prints the build root's export lines, and does nothing else |
| [`provision`](#provision) | Copies settings into a new worktree and allows its `.envrc` |
| [`disk`](#disk) | Prints the disk floors, each volume's free space and its state |
| [`rules`](#rules-and-facts) | Prints the rules, each with its id |
| [`facts`](#rules-and-facts) | Prints the facts the plan is made from, for debugging; its shape may move |
| [`hook reference-transaction`](#the-hook) | Logs each committed ref change, and starts `sweep --unattended` when the default branch's ref is among them |

### status

```text
git-worktree-manager status [OPTIONS] [worktree-or-branch]...
```

- `[worktree-or-branch]...`: worktrees or branches to narrow the output to, each by its path, its folder's name or its branch's name (default: all). A name that matches no worktree or branch is bad usage, and exits 2.
- `--repository <checkout>`, `--repositories <file>`, `--root <dir>`, `--default-branch <branch>`: see [What the commands share](#what-the-commands-share).
- `--size`: measure each worktree's bytes on disk.
- `--format <format>`: `text` (the default), `json` or `markdown`.

`status` is `sweep --dry-run`: one function builds both reports.

### sweep

```text
git-worktree-manager sweep [OPTIONS]
```

- `--unattended`: the mode the hook runs a sweep in; see [The sweep](#the-sweep).
- `--dry-run`: change nothing, and print what would be done.
- `--repository <checkout>`, `--repositories <file>`, `--root <dir>`, `--default-branch <branch>`.
- `--size`: measure each worktree's bytes on disk.
- `--format <format>`: `text` (the default), `json` or `markdown`.

See [The sweep](#the-sweep) for what it does, in order.

### finish

```text
git-worktree-manager finish [OPTIONS] <worktree>
```

- `<worktree>`: the worktree.
- `--abandon`: end it though its work hasn't landed: archive the branch, compare the archive, then remove and delete. It requires `--reason`.
- `--reason <text>`: why it is abandoned, logged with the archive. It requires `--abandon`.
- `--dry-run`: change nothing, and print what would be done.
- `--default-branch <branch>`: the default branch when `refs/remotes/origin/HEAD` names none; it counts only when `refs/remotes/origin/<branch>` holds a commit (see [What the commands share](#what-the-commands-share)).
- `--format <format>`: `text` (the default) or `json`.

`finish` applies the worktree and branch rules to one worktree without the `idle` test. With `--abandon` it also passes the `landed` test for a worktree on a branch: the branch is archived with the reason logged, the archive is compared, and only then is the worktree removed and the branch deleted. A detached `HEAD` that isn't merged doesn't pass `landed`, even with `--abandon`, so `finish` keeps a worktree on one whose folder exists, and exits 3. One whose folder is gone is pruned, as `worktree.prune` says: its `HEAD` is archived and compared first.

- **What it reads when:** unless given `--dry-run`, it finds its repository, takes the repository's `sweep.lock`, and only then reads the repository and plans. After the archives and their compare pass it reads and plans again, as a sweep does, and a step whose target changed fails. When a sweep holds the lock, `finish` says once on standard error that it waits, and waits. A sweep that found the lock held while `finish` had it is started once `finish` is done, as `sweep --unattended`, detached.
- **What it writes:** besides the worktree and branch, an `archived` line in the log for each archive it writes (see [The log](#the-log)), and the backup's READMEs when it archives.
- **Exits:** 0 when the worktree is ended, or planned under `--dry-run`; 1 when a step or an archive failed; 2 when the path is no worktree of its repository; 3 with the reasons on standard error when the worktree stays.
- **A worktree whose folder is gone** is found from the repository `finish` runs in, so run it inside that repository.

### archive

```text
git-worktree-manager archive write [OPTIONS] [branch]...
git-worktree-manager archive verify [OPTIONS]
git-worktree-manager archive list [OPTIONS] [branch]
git-worktree-manager archive restore [OPTIONS] <branch>
```

- **`write [branch]...`:** archives the branches named, or every unmerged branch whose tip the backup lacks. With no branch it is the one-time archive of everything, run by name before a first cleanup. Takes `--dry-run`, `--repository`, `--repositories`, `--root`, `--default-branch` and `--format text|json`.
- **`verify`:** runs the compare pass over every archive in the backup. Takes `--repository`, `--repositories`, `--root` and `--format text|json`.
- **`list [branch]`:** lists the backup's archives, newest first, or only this branch's. Takes `--repository` and `--format text|json`.
- **`restore <branch>`:** recreates the branch from its newest archive. It only creates a branch, and fails on one that exists. Takes `--at <time>`, `--dry-run`, `--repository` and `--format text|json`.
  - `--at` takes the last part of an archive's ref, such as `2026-11-18T091200Z` or `2026-11-18T091200Z-2`, and picks that archive.
  - Or it takes a time: seconds or milliseconds since the epoch, an RFC 3339 time such as `2026-11-18T09:12:00Z`, a date and time without a zone, read as UTC, or a time as `ps -o lstart=` prints it, read on the local clock. It picks the branch's newest archive written in that second.

### clean

```text
git-worktree-manager clean [OPTIONS] <worktree>
```

- `<worktree>`: the worktree's root folder.
- `--dry-run`: change nothing, and print what would be done.
- `--format <format>`: `text` (the default) or `json`.

It deletes the worktree's folder in the build root and its build output: every ignored entry that isn't a kept file (see [Kept files and build output](#kept-files-and-build-output)), never entering a nested repository, and then each folder that leaves empty. It leaves the main checkout and a locked worktree as they are, and exits 3 with the reason. A folder inside a worktree, rather than its root, is bad usage, and exits 2.

### env

```text
git-worktree-manager env [OPTIONS]
```

- `--languages <list>`: comma- or space-separated languages, by name or alias (default: detected from the files at the worktree's root). A word that names no language is bad usage, and exits 2.
- `--shell <shell>`: the syntax of the lines, `sh` (the default) or `fish`.
- `--repository <checkout>`: the repository's checkout (default: the one the command runs in).
- `--format <format>`: `text` (the default) or `json`.

Run it as `eval "$(git-worktree-manager env)"`. See [The build root](#the-build-root).

### provision

```text
git-worktree-manager provision [OPTIONS] [worktree]
```

- `[worktree]`: the worktree (default: the one the command runs in).
- `--format <format>`: `text` (the default) or `json`.

See [The build root](#the-build-root) for its steps.

### disk

```text
git-worktree-manager disk [OPTIONS]
```

- `--repository <checkout>`: the repository's checkout (default: the one the command runs in).
- `--format <format>`: `text` (the default) or `json`.

It reads, and creates and writes nothing. See [The disk floors](#the-disk-floors).

### rules and facts

```text
git-worktree-manager rules [--format text|json]
git-worktree-manager facts [OPTIONS]
```

- **`rules`** prints the rules below, each with its id, from the same data the rules' functions use.
- **`facts`** prints the facts the plan is made from, what git and the system said, typed, and the repository's settings. It takes `--repository`, `--default-branch`, `--size` and `--format text|json`, and prints JSON under either format. It is for debugging, and its shape may move.

### What the commands share

- **Which repository:** the one the command runs in, or `--repository <checkout>`, each found upward from the folder.
  - `status`, `sweep`, `archive write` and `archive verify` also take `--repositories <file>`, a JSON array of `{"path", "default_branch"}`, one per repository, with relative paths read against `--root <dir>`, or against the current folder without it.
  - A manifest entry is read as its own clone, at its own path, never as a repository that encloses it.
- **Git's variables:** finding the repository from the folder a command runs in or `--repository` reads `GIT_DIR` and `GIT_WORK_TREE`, as git does. It doesn't read `GIT_CEILING_DIRECTORIES` or `GIT_DISCOVERY_ACROSS_FILESYSTEM`: it finds a repository above a ceiling where git would stop, and it never searches across a filesystem boundary, even where that variable lets git.
  - A manifest entry, and every read of a repository once it is found, opens at its own path: no `GIT_*` variable chooses its repository, worktree, objects or config, `GIT_CONFIG_COUNT` and its keys included.
  - The repository's config, the user's `~/.gitconfig`, the XDG `git/config` and the system config are read at their usual places; a config moved with `GIT_CONFIG_GLOBAL` or `GIT_CONFIG_SYSTEM` is not.
  - The `git` children it runs, and the sweep it starts detached, run without git's local variables (such as `GIT_DIR` and `GIT_INDEX_FILE`) and without `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM` and `GIT_CONFIG_NOSYSTEM`, so what it plans and what it does read the same repository and config. Each `git` call also runs with `--no-optional-locks`.
  - An entry that holds none of a clone's parts (no `.git`, no `HEAD`, and no `objects` with `refs` or `packed-refs`) is reported as skipped. One that holds them but doesn't open, a broken bare clone included, is reported as unreadable. Either way the run goes on.
- **The default branch:** the target of `refs/remotes/origin/HEAD`, else the manifest's `default_branch`, else `--default-branch <branch>`. Each counts only when `refs/remotes/origin/<branch>` holds a commit, so `--default-branch main` on a clone without `origin` names none.
  - A repository with none is planned as "no default branch". Nothing in it is removed but the record of a worktree whose folder is gone and orphaned build output.
- **Acting:** `sweep`, `finish`, `clean`, `archive write` and `archive restore` act unless given `--dry-run`, under which they change nothing and print what they would do.
- **Output:** `--format text` (the default), `json` or `markdown`; `markdown` is for `status` and `sweep`. Text and Markdown are rendered from the JSON report by pure functions, and nothing parses them.
- **Sizes** on disk are measured only with `--size`.
- **Fetching** is the caller's: run `git fetch --prune` before a sweep.

### Exit statuses

| Status | Means |
| --- | --- |
| 0 | Done, or planned under `--dry-run`; a skipped repository is no failure |
| 1 | Failed: a repository couldn't be read, a step failed, an archive failed its compare, or a command that needs a repository ran outside one |
| 2 | Bad usage, with nothing on standard output |
| 3 | Kept: `finish` or `clean` left its target, with the reasons printed |

- **Outside a repository** a command that needs one exits 1: see [Errors](#errors).
- **The hook** exits 0 once its arguments parse, whatever else fails, so it never fails git.

## The JSON report

`status`, `sweep`, `finish`, `archive write` and `archive verify` print one report under `--format json`, so a caller parses one thing. Every other command prints a shape of its own: see [Other commands' JSON](#other-commands-json). `sweep --dry-run --format json` prints:

```json
{
  "schema": 1,
  "command": "sweep",
  "dry_run": true,
  "time": "2026-11-19T09:12:00Z",
  "repositories": [
    {
      "path": "/Users/…/app",
      "common_dir": "/Users/…/app/.git",
      "default": {
        "branch": "main",
        "ref": "refs/remotes/origin/main",
        "oid": "…"
      },
      "problem": null,
      "worktrees": [
        {
          "path": "/Users/…/app.worktrees/fix",
          "branch": "fix",
          "head": "…",
          "main": false,
          "action": "remove",
          "rule": "worktree.remove",
          "outcome": "planned",
          "archive": null,
          "tests": [
            { "name": "idle", "ok": true, "detail": "idle 3h of the 1h needed" }
          ],
          "reasons": [],
          "survives": [
            { "kind": "merged", "ref": "refs/remotes/origin/main", "oid": "…" }
          ],
          "lock": null,
          "attention": [],
          "bytes": null
        }
      ],
      "branches": [
        {
          "name": "fix",
          "tip": "…",
          "upstream": {
            "ref": "refs/remotes/origin/fix",
            "gone": true,
            "ahead": 0,
            "behind": 0
          },
          "state": "landed",
          "action": "delete",
          "rule": "branch.landed",
          "outcome": "planned",
          "archive": null,
          "worktree": "/Users/…/app.worktrees/fix",
          "reasons": [],
          "survives": [
            { "kind": "merged", "ref": "refs/remotes/origin/main", "oid": "…" }
          ],
          "attention": []
        }
      ],
      "backup": {
        "path": "…",
        "archived": 0,
        "verified": 0,
        "failed": 0,
        "compares": []
      },
      "rescues": [],
      "build_output": [],
      "totals": {
        "archives.failed": 0,
        "archives.planned": 0,
        "archives.verified": 0,
        "archives.written": 0,
        "attention": 0,
        "branches.delete": 1,
        "branches.keep": 0,
        "failed": 0,
        "problems": 0,
        "repositories": 1,
        "rescues": 0,
        "worktrees.keep": 0,
        "worktrees.prune": 0,
        "worktrees.remove": 1
      }
    }
  ],
  "totals": {
    "archives.failed": 0,
    "archives.planned": 0,
    "archives.verified": 0,
    "archives.written": 0,
    "attention": 0,
    "branches.delete": 1,
    "branches.keep": 0,
    "failed": 0,
    "problems": 0,
    "repositories": 1,
    "rescues": 0,
    "worktrees.keep": 0,
    "worktrees.prune": 0,
    "worktrees.remove": 1
  },
  "backup_note": null
}
```

- **`dry_run`:** true only when the run planned without acting: under `--dry-run` and `status`, and for a sweep that planned in some repository and acted in none, as `sweep --unattended` does where `worktreeManager.unattended` isn't `sweep`.
- **`action`:** `remove`, `prune` or `keep` for a worktree; `delete` or `keep` for a branch.
- **`outcome`:** `planned` under a dry run, else `done`, or `failed` with an `error`; `none` for `keep`.
- **`archive`:** null, or `{"ref", "state"}` with the state `planned`, `written`, `verified` or `failed`.
- **`survives`:** where the commit is besides this clone, each of the kind `merged`, `pushed` or `archived`. Empty means nowhere else.
- **`reasons` and `attention`:** `{"code", "detail"}`, with the codes `rules` prints.
- **`lock`:** `{"reason", "who", "pid", "since", "alive"}`.
- **`build_output`:** one `{"path", "worktree", "action", "outcome", "error"}` for each `wt/<id>/` of the build root that no listed worktree owns, the action `delete` or `keep`; `worktree` is null without a back-pointer.
- **`problem`:** `{"code", "detail"}` for a repository that is skipped, unreadable or has no default branch.
- **`backup.compares`:** one `{"ref", "oid", "ok", "detail"}` per archive the compare pass checked: the archive ref, the commit it must hold, whether every check passed, and which check failed and how, or null.
- **`rescues`:** one `{"branch", "tip", "archive", "outcome", "error"}` per branch deleted by hand that the rescue archives: the tip the log last saw, the archive as `{"ref", "state"}` or null, and `error` only when `outcome` is `failed`.
- **`backup_note`:** the last line of a run that wrote archives, which names how many and asks for a backup of `~/backups/`. Under `archive write --dry-run` it names the archives planned and not written. Else null.
- **`totals`:** under `status`, `sweep` and `finish`, each repository's and the run's hold every one of these counts, zeros included, the run's summed over its repositories. Under `archive write` and `archive verify` both are `{}`.

| Key | Counts |
| --- | --- |
| `repositories` | Repositories: 1 in a repository's totals |
| `problems` | Repositories skipped, unreadable or with no default branch |
| `worktrees.remove` | Worktrees removed, or to be under a dry run |
| `worktrees.prune` | Worktrees pruned, or to be |
| `worktrees.keep` | Worktrees kept |
| `branches.delete` | Branches deleted, or to be |
| `branches.keep` | Branches kept |
| `archives.planned` | Archives planned and not written: every archive under a dry run |
| `archives.written` | Archives written to the backup |
| `archives.verified` | Archives that passed the compare pass |
| `archives.failed` | Archives that failed it |
| `rescues` | Branches deleted by hand and archived from the log, or to be |
| `failed` | Removals, prunes, deletions, rescues and deletions of build output that failed |
| `attention` | Worktrees and branches with an attention code |

- **A change that breaks a reader** raises `schema`.

### Errors

Under `--format json`, a command that fails before it has a report prints `{"schema": 1, "command", "error": {"code", "detail"}}` on standard output, and says the detail on standard error too. Under `text` it says it only on standard error. The codes:

- `unreadable`: a repository or the manifest couldn't be read;
- `step-failed`: a step failed.

An archive that fails its compare is no error of this kind: it is reported in the report's `backup` and on its branch, and the run exits 1.

A command that needs a repository, `env` included, fails outside one with `not-a-repository`, which it says only on standard error, printing nothing on standard output. Three need none, and exit as anywhere else: `rules`; the hook; and `status`, `sweep`, `archive write` and `archive verify` given `--repositories <file>`, which report an entry that is no repository as skipped.

### Other commands' JSON

Each has `schema` and `command`, the command's name as it is typed, such as `"archive list"`.

| Command | Prints |
| --- | --- |
| `archive list` | `{"schema", "command", "path", "backup", "archives": [{"ref", "branch", "time", "oid", "compared"}]}`: the common git directory, the backup, and its archives, newest first; `compared` is null, as `list` checks none |
| `archive restore` | `{"schema", "command", "dry_run", "time", "path", "backup", "branch", "ref", "oid", "outcome", "error"}`: the archive the branch was made from, with `error` only when `outcome` is `failed` |
| `clean` | `{"schema", "command", "dry_run", "worktree", "outcome", "reasons", "build_output", "errors"}`: `outcome` is `none` when the worktree is kept, with `reasons`; `build_output` lists every path deleted, or to be, the worktree's folder in the build root first, then each file and link, then each folder left empty; `errors` holds `{"path", "error"}` for each deletion that failed |
| `env` | `{"schema", "command", "worktree", "languages", "exports": [{"name", "value"}]}`: the lines `env` prints as text, with `value` null for an unset |
| `provision` | `{"schema", "command", "worktree", "steps": [{"step", "outcome", "detail"}]}`: the steps `repo`, `worktree`, `never-index`, `time-machine`, `settings` and `direnv`, each `done`, `skipped` or `failed` |
| `disk` | `{"schema", "command", "floors": {"build_gb", "cleanup_only_gb"}, "disks": [{"path", "free_gb", "state"}]}`, the state `ok`, `below-build` or `cleanup-only` |
| `rules` | `{"schema", "command", "branch_rules": [{"id", "state", "when", "action", "reason"}], "worktree_tests": [{"name", "passes", "reason"}], "worktree_rules": [{"id", "when", "action"}], "reasons": [{"code", "text"}], "attention": [{"code", "on", "when"}]}` |
| `facts` | `{"schema", "command", "facts", "settings"}`, one object per repository, in either format |

## The rules

Each rule is a pure function over facts read beforehand, and `rules` prints them from the same data.

### Where commits survive

- **Merged:** the commit is an ancestor of the default branch's remote-tracking ref.
- **Archived:** an archive ref in the local backup points at it, and passes the check made when the facts are read: `git rev-parse <ref>` in the backup gives the commit, and `git rev-list --objects --missing=error <commit>` finds every object under it. That check runs no `git fsck`; the compare pass after an archive is written does.
- **Pushed:** a remote-tracking ref other than `HEAD` points at it. It is reported, and proves nothing: the ref is as old as the last fetch.
- **Nothing is deleted unless its tip is merged or archived.** A local ref, a tag or an archive ref inside the clone counts for nothing, since it goes when the clone does.
- **Content landed:** every path the branch changed since its merge base has, in the default branch's tree, the mode and object the branch gave it, or is absent there if the branch deleted it. The comparison is read-only.

### Branches

The first rule a branch matches gives its state and action.

| # | Rule id | State | When | Action |
| --- | --- | --- | --- | --- |
| 1 | `branch.default` | `default-branch` | It is the default branch | Keep |
| 2 | `branch.no-default` | `no-default-branch` | The repository's default branch is unknown | Keep |
| 3 | `branch.landed` | `landed` | Its tip is merged | Delete |
| 4 | `branch.disposable` | `disposable` | Its name matches a `disposable` pattern | Archive, then delete |
| 5 | `branch.content-landed` | `content-landed` | Its content has landed | Archive, then delete |
| 6 | `branch.upstream-gone` | `upstream-gone` | Its upstream is configured and gone | Keep; `finish --abandon` archives, then deletes |
| 7 | `branch.diverged` | `diverged` | It is both ahead of and behind its upstream | Keep; `finish --abandon` archives, then deletes |
| 8 | `branch.in-review` | `in-review` | Its upstream exists | Keep; `finish --abandon` archives, then deletes |
| 9 | `branch.local-only` | `local-only` | Otherwise | Keep; `finish --abandon` archives, then deletes |

- **A branch checked out in a worktree that stays is kept,** with the reason `checked-out`.
- **A symbolic ref under `refs/heads/`** is no branch, and is never planned. A worktree on one holds the branch at the end of its chain.
- **Archive, then delete** skips the archive when the tip is already merged or archived.
- **Deletion** is `git update-ref --no-deref -d refs/heads/<branch> <tip>`, with the tip the plan tested, so git refuses a branch that moved, and deleting a ref never deletes one it points at.

### Worktrees

The tests run in this order, and each failed test is a reason.

| # | Test | Passes when | Reason when it fails |
| --- | --- | --- | --- |
| 1 | `folder` | Its folder exists | `folder-missing` |
| 2 | `working` | The folder is a working worktree of this repository | `broken` |
| 3 | `not-main` | It is a linked worktree; a bare repository has no main worktree | `main-worktree` |
| 4 | `default-branch` | The default branch is known | `no-default-branch` |
| 5 | `unlocked` | It has no lock | `locked` |
| 6 | `landed` | Its `HEAD` is merged, or its branch is `disposable` or `content-landed` | `not-landed` |
| 7 | `clean` | Nothing is uncommitted, tracked or untracked | `uncommitted` |
| 8 | `keep-files` | It holds no kept file of its own | `keep-pattern-files` |
| 9 | `idle` | Its `HEAD`, `logs/HEAD` and index have been unchanged for `idle` | `not-idle` |

The first worktree rule that applies gives the action:

| # | Rule id | When | Action |
| --- | --- | --- | --- |
| 1 | `worktree.remove` | Every test passes | `git worktree remove <path>`, never with `--force` |
| 2 | `worktree.prune` | The folder is gone, and it is unlocked and linked, whether or not the default branch is known | Archive a `HEAD` that is neither merged nor archived, then remove the worktree's record once that archive passed its compare |
| 3 | `worktree.keep` | Anything else | Keep |

- **A prune** archives a detached `HEAD` under a name that says so, and removes one worktree's record at a time with `git worktree remove <path>` on the missing folder, so a worktree whose archive failed stays registered.
- **A missing folder whose `HEAD` can't be read** stays registered, kept with the reason `broken`, since nothing says which commit to archive before its record goes. One whose `HEAD` is on a branch that doesn't exist has no commit to lose, and is pruned.
- **A lock is never broken.** Whether its process is alive is told from `ps -o lstart= -p <pid>`, by the form of its reason:
  - Claude Code's `claude agent <id> (pid <pid> start <time>)`: alive when that pid's process started at that time, within 2 seconds.
  - `<who> since <time>`: the pid comes from a `pid <n>` in the reason, and the process is alive when it started no later than the `since` time plus 2 seconds.
  - Any other reason, such as `held by pid 42`, or a `since` reason whose time can't be read: the pid comes from a `pid <n>` in it, and a running process with that pid is alive.
  - A reason with no pid has no liveness.

### Kept files and build output

One function splits a worktree's ignored entries, and both the `keep-files` test and `clean` use it.

- **Kept:** an ignored entry a keep pattern matches. The patterns are the built-in list, then the repository's `.worktree-keep` and `.worktreeinclude`, in gitignore syntax, matched by `gix-ignore`; a later pattern wins, so `!` undoes an earlier one.
- **Its own:** a kept file whose bytes differ from the main checkout's copy, or that the main checkout lacks, and every nested repository.
- **Build output:** every other ignored entry. `clean` deletes these and nothing else.

The built-in keep patterns: `.env`, `.env.*`, `.envrc`, `.envrc.local`, `*.local.*`, `.claude/settings.local.json`, `.idea/`, `*.iml`, `.superpowers/`, `HANDOFF.md`, `*.tfplan`, `tfplan*`, `*.tfstate*`, `.xtdb/`, `.nrepl-port`, `*-workspace/`, `.gstack/`, `reports/`, `.worktree-keep` and `.worktreeinclude`.

### Attention

What a person must settle, as codes on the item:

| Code | On | When |
| --- | --- | --- |
| `uncommitted` | Worktree | A linked worktree has uncommitted changes |
| `locked-landed` | Worktree | A landed worktree has been locked longer than `lockReport` |
| `broken` | Worktree | It fails `working` |
| `local-only-old` | Branch | A `local-only` branch is older than `localOnlyReport` |
| `diverged` | Branch | It is in the `diverged` state |
| `upstream-gone` | Branch | It is in the `upstream-gone` state |
| `archive-failed` | Branch | Its archive failed its compare |

## The sweep

Each repository is swept under `flock` on `<state>/sweep.lock`.

- **A sweep a person runs** waits for the lock when another sweep or `finish` holds it, saying so once on standard error, then acts.
- **A sweep with `--unattended`** that finds the lock held leaves `sweep.pending`, reports the repository as skipped, and goes on; it exits 0.
- **Whoever holds the lock** looks for `sweep.pending` when it ends. A sweep runs over the repository again, in its own mode, up to 10 rounds in all, and its report covers every round, keeping a round's failure that a later round didn't undo. `finish` starts `sweep --unattended` detached. A `sweep.pending` the sweep can't clear ends its rounds.

In each repository, in order:

1. **Archive** every branch the plan deletes, and every pruned worktree's `HEAD`, that needs it, in one `git push --no-verify <backup> <tip>:refs/archive/<branch>/<time>...`.
2. **Compare** those archives in the backup. A branch whose archive fails stays, with its worktree; a worktree whose archive fails isn't pruned; and the run exits 1.
3. **Prune** worktrees whose folder is gone.
4. **Remove** worktrees.
5. **Delete** branches.
6. **Delete orphaned build output:** a `wt/<id>/` of the build root whose `WORKTREE` back-pointer names a folder git no longer lists.
7. **Rescue** each branch deleted by hand whose last logged tip still exists and is reachable from no ref, unless an archive in the backup at that tip passes the check made when facts are read (see [Where commits survive](#where-commits-survive)): it is archived from that tip, and compared.
8. **Write** the report to `<state>/last-sweep.json`.

- **After the compare pass** the repository is read and planned again, and a prune, removal or deletion whose target changed since the first plan fails and is never retried.
- **Orphaned build output** is listed by the dry run before a sweep deletes it.
  - A listed worktree's `wt/<id>/`, matched by its id, its real path or the file identity of the folder its back-pointer names, is never touched.
  - A back-pointer that is absent, relative, cut short (with no final newline) or unreadable is reported, not deleted; one that can't be read fails only when the sweep acts.
  - While a listed worktree's folder can't be identified, nothing is deleted in that repository, since any `wt/<id>/` might be its own.
- **Not in a sweep:** no backfill, no cleaning of a listed worktree, and no fetch.
- **`--unattended`,** which the hook passes, acts only where the repository's `worktreeManager.unattended` is `sweep`; otherwise it plans. Either way it writes the report. Its handling of a held lock is above.
- **The last line** of a run that wrote archives is the report's `backup_note` (see [The JSON report](#the-json-report)).

### The dry run

`--dry-run` and `status` write nothing: no ref, no object, no index refresh, no lock, no log line and no state file. Repositories are read through gitoxide, and the reads that run `git` take no optional lock (see [What the commands share](#what-the-commands-share)). The tests compare refs, the worktree list, every file's mtime in the repository and the state folder before and after.

## The local backup

- **Where:** one bare repository per project, `~/backups/git-archive/<owner>/<repo>.git`. The owner and repository come from `origin`'s URL, or are `local/<folder>` for a clone without one. It is made with `git init --bare` the first time. Nothing is pushed to any remote, so a branch that was never pushed stays private, even in a public fork.
- **An archive** is `refs/archive/<branch>/<UTC time>`, to the second, with `-2`, `-3` added to a name that is taken. The push can only create an archive, never move one.
- **The compare pass:** `git rev-parse <ref>` in the backup equals the tip, `git rev-list --objects --missing=error <tip>` passes, and `git fsck --connectivity-only` passes, once per compare pass. Every archive written is compared before anything it protects is deleted.
- **READMEs** at `~/backups/git-archive/` and in each owner folder say what is there and how to restore it.
- **Restoring:** `archive restore <branch>` fetches the archive's objects from the backup, writing no ref and no `FETCH_HEAD`, then creates the branch with `git update-ref`. By hand, from a clone of the project: `git fetch ~/backups/git-archive/<owner>/<repo>.git 'refs/archive/*:refs/archive/*'`, then `git branch <branch> refs/archive/<branch>/<time>`.

## The hook

`hook reference-transaction <state>` is git's `reference-transaction` hook.

- **In any state but `committed`** it returns at once.
- **On `committed`** it opens no repository and runs no `git`. It appends one JSON line per ref to the log, in `write`s of whole lines under 4 KB each, so lines from hooks running at once never interleave. When the default branch's ref is among the refs, it starts `sweep --unattended --repository <the folder git ran it in>` detached, in `/`, since that folder may be a worktree the sweep removes, in a process group of its own, with its standard streams on `/dev/null`.
- **A symbolic side** (`ref:<target>`) is logged as 40 zeros, so a ref that turns symbolic is logged like a deletion, and one that stops being symbolic like a creation. A change with both sides symbolic, or symbolic and null, and a blank line are passed over; a line of git's input that isn't `<old> <new> <ref>` is said on standard error.

### The log

The log, `events.jsonl` in the repository's state folder, is JSON Lines, one object per line:

| Field | Holds |
| --- | --- |
| `time` | When the line was written, an RFC 3339 UTC time, such as `2026-11-19T09:12:00.501613Z` |
| `pid` | The process that wrote it |
| `event` | `ref` for a ref change the hook saw, `archived` for an archive a sweep or `finish` wrote |
| `ref` | The ref changed, or the archive ref written |
| `old`, `new` | On a `ref` line: the ref's object names before and after, each 40 hex digits; 40 zeros for a creation's `old`, a deletion's `new`, an `old` git didn't give, and a symbolic side |
| `ref_bytes` | On a `ref` line whose ref name was cut to keep the line under 4 KB: the name's whole length in bytes. The cut name ends in `...`, which no real ref name can |
| `branch`, `tip`, `reason` | On an `archived` line: the branch, the commit archived, and `finish --abandon`'s reason or null |

The branches deleted, or turned into symbolic refs, with the tip each had before:

```sh
jq -c 'select(.event == "ref" and (.ref | startswith("refs/heads/")) and .new == ("0" * 40)) | {time, ref, old}' events.jsonl
```

### Installing the hook

No command installs the hook: until its shim is in a repository's hooks folder, nothing logs that repository's ref changes, and no sweep starts by itself there. The shim is three lines of `sh` that `exec` the binary with `WORKTREE_MANAGER_LOG` and `WORKTREE_MANAGER_DEFAULT_REF` set; the library's `hook::shim` builds the same text. For a clone of `app` whose default branch is `main`, it is:

```sh
#!/bin/sh
[ "$1" = committed ] && [ -x '/nix/store/…-git-worktree-manager-0.2.0/bin/git-worktree-manager' ] || exit 0
WORKTREE_MANAGER_LOG='/Users/…/.local/state/git-worktree-manager/app-1a2b3c4d/events.jsonl' WORKTREE_MANAGER_DEFAULT_REF='refs/remotes/origin/main' exec '/nix/store/…-git-worktree-manager-0.2.0/bin/git-worktree-manager' hook reference-transaction "$@"
```

To install it, run this in the repository, with paths that hold no single quote:

```sh
common=$(cd "$(git rev-parse --git-common-dir)" && pwd -P)
name=$(basename "${common%/.git}" .git)
key="$name-$(printf %s "$common" | shasum -a 256 | cut -c1-8)"
log="${XDG_STATE_HOME:-$HOME/.local/state}/git-worktree-manager/$key/events.jsonl"
program=$(command -v git-worktree-manager)
default=$(git symbolic-ref refs/remotes/origin/HEAD)
hook="$(git rev-parse --git-path hooks)/reference-transaction"
printf '%s\n' '#!/bin/sh' \
  "[ \"\$1\" = committed ] && [ -x '$program' ] || exit 0" \
  "WORKTREE_MANAGER_LOG='$log' WORKTREE_MANAGER_DEFAULT_REF='$default' exec '$program' hook reference-transaction \"\$@\"" \
  > "$hook"
chmod a+x "$hook"
```

It names the binary by its path, so after the binary moves, as with a new Nix store path, run it again.

A failing hook aborts git's transaction in the `preparing` and `prepared` states, so the shim starts the binary only on `committed`, where git ignores the exit status, and exits 0 when the binary is gone, as after a Nix garbage collection.

What the hook costs:

- **The binary:** `test_a_1000_ref_transaction_costs_the_hook_under_5_ms` in `tests/hook.rs` runs the release binary on a 1,000-ref transaction in each state, takes the fastest of 15 runs of the whole process, from its start to its exit, and fails when the `committed` run takes 5 ms or more.
- **The shim on a fetch:** `test_the_shim_adds_under_5_ms_to_a_1000_ref_fetch` times a 1,000-ref fetch with no hook, with an empty hook and with the shim, and fails when the shim adds 5 ms plus two process starts, its `sh` and the binary, over the empty hook.
- **Both are timing budgets,** ignored by default and run by name in a release build: `cargo test --release --test hook -- --ignored --nocapture`.
- **Git runs the hook once for each state of a transaction,** so a fetch starts the shim's `sh` two or three times and the binary once, which no test bounds.

## The build root

Each repository's build output goes under its build root, `${XDG_CACHE_HOME:-~/.cache}/git-worktree-manager/<repo>-<hash8>/`:

- **`shared/`:** caches every worktree can share;
- **`wt/<id>/`:** one worktree's own output, with the id `main` for the main checkout, else the folder's name and the first 8 hex digits of the SHA-256 of its path;
- **`REPO`:** the repository's common git directory, which `provision` writes;
- **`wt/<id>/WORKTREE`:** the back-pointer, the worktree's real path and one newline, which `provision` writes and the sweep reads.

[`clean`](#clean) deletes one worktree's `wt/<id>/` and the build output inside the worktree.

### env

`env` prints export lines and writes nothing:

- `WORKTREE_MANAGER_BUILD_ROOT` and `WORKTREE_MANAGER_BUILD_DIR`, the build root and the worktree's `wt/<id>/`;
- each language's variables, below;
- `GRADLE_OPTS` with `-Dorg.gradle.caching=true` added once, for Gradle;
- `WORKTREE_MANAGER_EXPORTED`, the languages' variables it set, with `:` between them.

A variable the environment already sets is a person's and stays as it is, unless `WORKTREE_MANAGER_EXPORTED` names it; one it set before that no language wants now is unset.

| Language | Aliases | Detected from | Variables |
| --- | --- | --- | --- |
| `c` | `c++`, `cpp` | `CMakeLists.txt`, `meson.build`, `configure.ac` | `CCACHE_DIR` = `shared/ccache`, `CCACHE_BASEDIR` = the worktree |
| `clojure` | `babashka` | `deps.edn`, `project.clj`, `bb.edn`, `shadow-cljs.edn` | — |
| `dotnet` | `csharp` | `*.sln`, `*.csproj`, `*.fsproj` | — |
| `go` | — | `go.mod` | — |
| `gradle` | `java`, `kotlin` | `build.gradle`, `build.gradle.kts`, `settings.gradle*`, `gradlew` | `GRADLE_OPTS` only |
| `javascript` | `js`, `ts`, `typescript` | `package.json` | — |
| `maven` | — | `pom.xml` | — |
| `nix` | — | `flake.nix`, `default.nix`, `shell.nix` | — |
| `python` | — | `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements*.txt`, `uv.lock` | `PYTHONPYCACHEPREFIX` = `shared/pycache`, `UV_PROJECT_ENVIRONMENT` = `wt/<id>/venv`, `RUFF_CACHE_DIR` = `wt/<id>/ruff`, `MYPY_CACHE_DIR` = `wt/<id>/mypy`, and `UV_LINK_MODE=clone` on macOS |
| `rust` | — | `Cargo.toml` | `CARGO_BUILD_BUILD_DIR` = `shared/cargo-build`, `CARGO_TARGET_DIR` = `wt/<id>/target` |
| `scala` | — | `build.sbt`, `build.mill`, `build.sc` | — |
| `swift` | — | `Package.swift` | `CLANG_MODULE_CACHE_PATH` = `shared/clang-modules` |
| `terraform` | `opentofu`, `tofu` | `*.tf` | `TF_DATA_DIR` = `wt/<id>/tf` |
| `xcode` | — | `*.xcodeproj`, `*.xcworkspace` | — |

### provision

`provision` reports each step's failure, and none stops the rest:

- it copies the main checkout's `.claude/settings.local.json` when the worktree has none;
- it runs `direnv allow` when the worktree's `.envrc` is byte for byte the checkout's and `direnv` is on the `PATH`;
- it writes the `WORKTREE` back-pointer into the worktree's `wt/<id>/` and `REPO` into the build root;
- on macOS it keeps the folder holding every build root, `${XDG_CACHE_HOME:-~/.cache}/git-worktree-manager/`, out of Spotlight with `.metadata_never_index`, and out of Time Machine with `tmutil addexclusion` when `tmutil` is on the `PATH`, then writes the marker `.excluded-from-time-machine` there, so a later run skips the exclusion.

A step it skips, such as `direnv` when it isn't on the `PATH`, is reported as `skipped` with the reason.

### The disk floors

`disk` reports the volumes holding the build root, the checkout (a bare repository's common git directory), the backup and `/nix` when it exists, each once.

| State | Free space |
| --- | --- |
| `ok` | At or above the build floor |
| `below-build` | Under the build floor |
| `cleanup-only` | Under the cleanup-only floor |

The floors, in GB of 10^9 bytes, are `WORKTREE_MANAGER_FLOOR_BUILD_GB` and `WORKTREE_MANAGER_FLOOR_CLEANUP_ONLY_GB`, else the same keys' `KEY=value` lines in the floors file, else 70 and 35. In the file a line starting with `#` is a comment, one pair of quotes around a value is dropped, and the last line of a key wins. A build floor below the cleanup-only floor is a failure.

## Names

All but the backup derive from the program's name, written once, in `Cargo.toml`.

| What | Where |
| --- | --- |
| State | `${XDG_STATE_HOME:-~/.local/state}/git-worktree-manager/<repo>-<hash8>/`, holding `events.jsonl`, `sweep.lock`, `sweep.pending` and `last-sweep.json` |
| Build root | `${XDG_CACHE_HOME:-~/.cache}/git-worktree-manager/<repo>-<hash8>/` |
| Floors file | `${XDG_CONFIG_HOME:-~/.config}/git-worktree-manager/floor` |
| Backup | `~/backups/git-archive/` |
| Variables | `WORKTREE_MANAGER_LOG`, `WORKTREE_MANAGER_DEFAULT_REF`, `WORKTREE_MANAGER_FLOOR_BUILD_GB`, `WORKTREE_MANAGER_FLOOR_CLEANUP_ONLY_GB`, `WORKTREE_MANAGER_BUILD_ROOT`, `WORKTREE_MANAGER_BUILD_DIR` and `WORKTREE_MANAGER_EXPORTED` |

`<repo>-<hash8>` is the repository's name and the first 8 hex digits of the SHA-256 of its common git directory's real path, so every worktree of one repository shares one folder.

Git config sets these per repository, read from the config files named in [What the commands share](#what-the-commands-share). A value that can't be read keeps the default. The durations take a number, whole or decimal, with `s`, `m`, `h`, `d` or `w`, or none for seconds, such as `90`, `30m`, `1.5d` or `2w`.

| Key | Default | Means |
| --- | --- | --- |
| `worktreeManager.unattended` | `report` | `sweep` lets a sweep the hook starts act; `report` only plans and writes the report |
| `worktreeManager.idle` | `1h` | How long a worktree's `HEAD`, `logs/HEAD` and index must be unchanged to pass `idle` |
| `worktreeManager.lockReport` | `1d` | How long a landed worktree may stay locked before it needs attention |
| `worktreeManager.localOnlyReport` | `7d` | How old a `local-only` branch's tip may be before it needs attention |
| `worktreeManager.disposable` | `preview/*` | Branch patterns in git's glob syntax, one per value, every value counted; any value replaces the default, and an empty one only clears it |

## Reads and writes

Repositories are read through gitoxide (`gix`): config, refs, worktrees and their locks, status, ignored entries, ancestry, merge bases and tree diffs, and the backup's archive refs.

These reads stay on `git` or `ps`:

| Read | Command | Where |
| --- | --- | --- |
| An archive ref's object in the backup, for its facts, the rescue's check and the compare pass | `git rev-parse` | `src/backup.rs`, `src/archive.rs` |
| Whether the backup holds every object under a tip, for its facts, the rescue's check and the compare pass | `git rev-list --objects --missing=error` | `src/backup.rs`, `src/archive.rs` |
| The backup's connectivity, in the compare pass | `git fsck --connectivity-only` | `src/archive.rs` |
| The worktrees git lists, just before orphaned build output is deleted | `git worktree list --porcelain -z` | `src/sweep/build_output.rs` |
| A worktree's `HEAD` just before its removal | `git rev-parse --verify HEAD` | `src/sweep/act.rs` |
| Whether a rescue's tip still exists | `git cat-file -e <tip>^{commit}` | `src/sweep/rescues.rs` |
| Whether any ref reaches a rescue's tip | `git rev-list --max-count=1 <tip> --not --all` | `src/sweep/rescues.rs` |
| When a lock's process started | `ps -o lstart= -p <pid>` | `src/system/real.rs` |

Every change to refs, worktrees and the backup goes through `git`: `worktree remove`, `update-ref -d`, `init --bare` and `push` into the backup, and `fetch` and `update-ref` for a restore. Each `git` call runs as [Git's variables](#what-the-commands-share) says.

Files change without `git`: `clean` and the sweep's orphaned-build-output step delete files, the hook, the sweep and `finish` append to the log and write the state folder, `archive write`, the sweep and `finish` write the backup's READMEs, and `provision` writes files and runs `direnv` and `tmutil`.

## Development

See `AGENTS.md`.
