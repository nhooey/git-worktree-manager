# nix-worktrees

Keeps git worktrees' build output outside them and shared, provisions them, removes finished ones and keeps builds off a full disk.

It holds one program, `worktrees`: the one implementation of a worktree's build environment, its provisioning, the cleaning of its build output, its removal and its listing. Dev shells, login shells, direnv, agent sessions and people all call it the same way, and it runs without Nix.

## Install and run

- **With Nix:** `nix run github:nhooey/nix-worktrees -- status`, or the `worktrees` package (`packages.<system>.default`) in a profile or a Home Manager configuration.
- **Without Nix:** from a clone, `python3 -m worktrees`, with the clone on `PYTHONPATH` or as the current directory; or `pip install .` or `uv tool install .`. It needs Python 3.9 or later and nothing outside the standard library.
- **It runs** `git` (2.31 or later), `ps`, `lsof`, `df` and `du`, and `direnv` and macOS's `tmutil` when they are there.

## The commands

Every command takes `--help`, long options only, and `--json` for output a program reads. A command that removes anything lists what it would do, and does it only with `--apply`.

| Command                                                         | Does                                                                                                                                                                                                                                                                |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `worktrees env [--languages <list>] [--shell <sh\|fish>]`       | prints export lines for the shared build caches of the checkout or worktree it runs in, provisioning the worktree first when it hasn't been; outside git it prints nothing                                                                                         |
| `worktrees provision [<worktree>]`                              | copies the main checkout's `.claude/settings.local.json` into the worktree when it has none, runs `direnv allow` when the worktree's `.envrc` is byte for byte the checkout's, and writes a marker in the worktree's git directory                                   |
| `worktrees clean <worktree> [--apply]`                          | deletes a finished agent's build output: the worktree's folder under the build root, and the gitignored files in the worktree except the kept ones                                                                                                                  |
| `worktrees finish <worktree> [--log <path>] [--apply]`          | removes the worktree and its branch when the branch has landed and the tree is clean; for a failed run, when `--log` names a saved log outside the worktree and the branch has no commits of its own; otherwise says why it kept them                                |
| `worktrees sweep [--all] [--apply] [--cargo-cap <GB>]`          | the repository's sweep, or every clone's under the configured roots with `--all`: prune, clear stale Claude Code locks, finish what has landed, clean idle worktrees, report unmerged ones, delete orphan build folders, cap the Cargo build folder, delete landed branches |
| `worktrees status [--all] [--no-size]`                          | one row per worktree: branch, landed or not, uncommitted paths, provisioned or not, lock, age and size; then the build root's size and the free space against the disk floors                                                                                      |
| `worktrees sessions [<checkout>]`                               | the Claude Code sessions of a checkout and all its worktrees, newest first                                                                                                                                                                                          |

`finish` and `sweep` run `git fetch --prune` first when the repository has a remote; `--no-fetch` skips it.

Exit status: 0 done, or listed without `--apply`; 1 failed; 2 bad usage; 3 kept or refused, by `finish` and `clean`, with the reasons printed.

### Loading the environment

```sh
eval "$(worktrees env)"            # sh, bash, zsh
worktrees env --shell fish | source # fish
```

## The build root

Every worktree's build output goes under one folder per repository, outside every worktree and outside `.git`:

```text
${XDG_CACHE_HOME:-$HOME/.cache}/worktree-builds/
  <repo>-<hash8>/          one per repository
    REPO                   the repository's common git directory, for the sweep
    shared/cargo-build/    CARGO_BUILD_BUILD_DIR, shared by every worktree
    shared/pycache/        PYTHONPYCACHEPREFIX
    shared/clang-modules/  CLANG_MODULE_CACHE_PATH
    shared/ccache/         CCACHE_DIR
    wt/<wt-id>/            what can't be shared: target/ swiftpm/ xcode/ venv/ ruff/ mypy/ tf/
    wt/<wt-id>/WORKTREE    the worktree's path, for the sweep
```

- **`<hash8>`** is the first 8 hex digits of the SHA-256 of the real path of `git rev-parse --path-format=absolute --git-common-dir`, so every worktree of a clone shares one folder and two clones with the same name don't.
- **`<wt-id>`** is the worktree folder's name and the first 8 hex digits of the SHA-256 of its path; the main checkout's is `main`.
- **On macOS** the first `env` that creates `worktree-builds/` excludes it from Time Machine with `tmutil addexclusion` and from Spotlight with a `.metadata_never_index` file.

## The variables

`WORKTREE_BUILDS_ROOT` (the repository's folder, `$ROOT` below) and `WORKTREE_BUILDS_WT` (the worktree's, `$WT`) are always set, for scripts whose tools take a flag rather than a variable. `WORKTREE_BUILDS_SET` names the variables `env` set.

- **A variable already set is left alone,** unless `env` set it itself, as `WORKTREE_BUILDS_SET` records, for another worktree than the one `WORKTREE_BUILDS_WT` names now; then it is recomputed. A subagent that inherits its parent's environment gets its own worktree's `CARGO_TARGET_DIR`, and a variable a person set stays theirs. Names `env` set that the current worktree doesn't want are unset.
- **Languages** come from `--languages`, comma-separated, or from the files at the worktree's root.

| Language and tool                    | Detected by                                         | What `env` sets                                                                                                            | What stays per worktree, and why                                     |
| ------------------------------------ | --------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `rust`: Cargo                        | `Cargo.toml`                                        | `CARGO_BUILD_BUILD_DIR=$ROOT/shared/cargo-build`, `CARGO_TARGET_DIR=$WT/target`                                            | the final binaries, which are small, so no worktree overwrites another's |
| `swift`: SwiftPM                     | `Package.swift`                                     | `CLANG_MODULE_CACHE_PATH=$ROOT/shared/clang-modules`; commands pass `--scratch-path "$WORKTREE_BUILDS_WT/swiftpm"`          | the build products, which hold absolute paths                        |
| `xcode`: Xcode                       | `*.xcodeproj`, `*.xcworkspace`                      | nothing; scripts pass `-derivedDataPath "$WORKTREE_BUILDS_WT/xcode"`                                                       | DerivedData, which is per project path                               |
| `javascript`: pnpm, bun, npm         | `package.json`                                      | nothing: pnpm and bun link `node_modules` from one store; prefer them, or `node_modules` from Nix, over npm               | `node_modules`, which follows the branch's lock file                 |
| `gradle`: Java and Kotlin            | `build.gradle*`, `settings.gradle*`, `gradlew`      | `GRADLE_OPTS` gains `-Dorg.gradle.caching=true`                                                                            | `build/` and `.gradle/`, refilled from Gradle's build cache          |
| `maven`: Java                        | `pom.xml`                                           | nothing: `~/.m2` is already shared                                                                                         | `target/`                                                            |
| `python`: uv, pip, ruff, mypy        | `pyproject.toml`, `setup.py`, `requirements*.txt`, `uv.lock` | `PYTHONPYCACHEPREFIX=$ROOT/shared/pycache`, `UV_PROJECT_ENVIRONMENT=$WT/venv`, `RUFF_CACHE_DIR=$WT/ruff`, `MYPY_CACHE_DIR=$WT/mypy`, and `UV_LINK_MODE=clone` on macOS | the virtual environment, whose editable installs point at one worktree |
| `go`                                 | `go.mod`                                            | nothing: `GOCACHE` and `GOMODCACHE` are already shared                                                                     | nothing                                                              |
| `terraform`: Terraform and OpenTofu  | `*.tf`                                              | `TF_DATA_DIR=$WT/tf`                                                                                                       | the providers and module links of one working directory              |
| `clojure`: Clojure, Babashka         | `deps.edn`, `project.clj`, `bb.edn`, `shadow-cljs.edn` | nothing: `~/.m2` and `~/.gitlibs` are already shared                                                                     | `.cpcache/`, `.shadow-cljs/`                                         |
| `dotnet`: C#                         | `*.sln`, `*.csproj`, `*.fsproj`                     | nothing: NuGet packages are already shared                                                                                 | `obj/` and `bin/`, whose `project.assets.json` holds absolute paths  |
| `scala`: sbt, Mill                   | `build.sbt`, `build.mill`, `build.sc`               | nothing: the Ivy and Coursier caches are already shared                                                                    | `target/`, `out/`                                                    |
| `c`: C and C++                       | `CMakeLists.txt`, `meson.build`, `configure.ac`     | `CCACHE_DIR=$ROOT/shared/ccache`, `CCACHE_BASEDIR` set to the worktree, so paths match across worktrees                    | the build folder                                                     |
| `nix`                                | `flake.nix`, `default.nix`, `shell.nix`             | nothing: the store is shared                                                                                               | `result` links, which go with the worktree                           |

`--languages` also takes `java`, `kotlin`, `typescript`, `ts`, `js`, `cpp`, `c++`, `csharp`, `opentofu`, `tofu` and `babashka`.

## Provisioning

The first `env` in a worktree, or `worktrees provision`, writes `REPO` and `WORKTREE` under the build root, copies `.claude/settings.local.json` from the main checkout when the worktree has none (copied, not linked, so an approval given in a worktree never widens the checkout's), runs `direnv allow` when the worktree's `.envrc` is byte for byte the checkout's, and writes `worktrees-provisioned` in the worktree's git directory. It is idempotent, and a failure in it never stops `env`'s exports: the reason goes to standard error.

## The rules

- **Never the main checkout,** known by being the first entry of `git worktree list --porcelain`.
- **Never `--force`:** removal is `git worktree remove <path>`, then `git branch --delete <name>`, so git itself refuses a dirty tree or an unmerged branch. Before deleting a branch, `finish` records the ref it landed in as its upstream, so git can check it.
- **Landed** means the branch's tip is an ancestor of the default branch, of `integration` where there is one, or of their remote-tracking branches; failing that, `git cherry` prints only `-` lines, which covers most squash and rebase merges. `gh` is never asked.
- **In use, so kept:** a lock by a live Claude Code process (its reason is `claude agent … (pid <pid> start <time>)` and that pid started at that time), a lock with any other reason, which is a person's, any process with its working directory inside (`lsof`), and, for the sweep, a Claude Code transcript for the worktree written in the last 30 minutes. A Claude Code lock whose pid is gone or started at another time is stale, and the sweep clears it.
- **Kept and reported:** uncommitted changes, commits that haven't landed, stashes on the branch, and gitignored files matching a keep pattern that the main checkout doesn't hold with the same bytes.
- **Copied settings never keep a worktree:** a kept file identical to the main checkout's is not the worktree's own.

### What `clean` keeps

`clean` deletes what `git clean -ndX` lists, except files matching a keep pattern, from three sources:

- the defaults: `.env`, `.env.*`, `.envrc`, `.envrc.local`, `*.local.*`, `.claude/settings.local.json`, `.idea/`, `*.iml`, `.superpowers/`, `HANDOFF.md`, `*.tfplan`, `tfplan*`, `*.tfstate*`, `.xtdb/`, `.nrepl-port`, `*-workspace/`, `.gstack/`, `reports/`;
- the repository's `.worktreeinclude`, the files Claude Code copies into a new worktree;
- the repository's own `.worktree-keep`, tracked, in gitignore syntax, for anything the defaults miss.

A folder `git clean` would delete whole that holds a kept file, or a nested repository, is entered, and only its own ignored, unkept files go. Every path is logged to `~/.local/state/worktrees/clean.log` before it is deleted. Nested repositories, worktrees inside the worktree among them, are never entered.

## Settings

`worktrees/defaults.json` lists every setting with its default. `${XDG_CONFIG_HOME:-~/.config}/worktrees/config.json` names only what it changes and is merged over it: objects merge, lists and values replace. An unknown key is reported on standard error.

| Setting                    | Default        | Means                                                              |
| -------------------------- | -------------- | ------------------------------------------------------------------ |
| `roots`                    | `["~/git"]`    | where `--all` looks for clones                                     |
| `scan_depth`               | `4`            | how many folders deep under each root it looks                     |
| `cargo_cap_gb`             | `60`           | the size past which the sweep deletes `shared/cargo-build/`        |
| `transcript_idle_minutes`  | `30`           | how recent a transcript keeps a worktree from the sweep            |
| `sweep_lock_stale_minutes` | `60`           | when a sweep's lock on `<root>/.sweep.lock` is taken over          |
| `fetch_timeout_seconds`    | `60`           | the limit on `git fetch`                                           |
| `keep`                     | the list above | the default keep patterns                                          |
| `sweep.apply`              | `false`        | for the plugin's hooks: whether their sweep passes `--apply`       |

### The disk floors

`status` compares the free space with two floors, read from `WORKTREES_FLOOR_BUILD_GB` and `WORKTREES_FLOOR_CLEANUP_ONLY_GB`, else from the same `KEY=value` lines in `${XDG_CONFIG_HOME:-~/.config}/worktrees/floor`, else 70 and 35 GB.

## Logs

- `~/.local/state/worktrees/clean.log`: every path `clean` deletes, before it goes.
- `~/.local/state/worktrees/sweep.log`: each sweep's verdicts, removals, orphans, the Cargo cap, and once a day per repository the unmerged worktrees.

## Development

Enter the dev shell with `nix develop` (or direnv). Then:

- `worktrees`: the program, from this checkout.
- `tests`: the core's and the commands' tests, `python3 -m unittest`; without Nix, `python3 -m unittest discover --start-directory worktrees/tests --top-level-directory .`.
- `check`: `nix flake check`: formatting and lints (treefmt with nixfmt and ruff), the package build and the tests.

The program is pure functions in `worktrees/core.py`, a thin shell around git, `ps`, `lsof`, `df`, `du` and the file system in `worktrees/system.py` and `worktrees/repo.py`, and the commands in `worktrees/cli.py`. The command tests build temporary git repositories with `HOME` and the XDG folders inside them; nothing is removed outside them.

## Licence

Apache-2.0; see `LICENSE`.
