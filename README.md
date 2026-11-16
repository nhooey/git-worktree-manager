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

## In a repository's dev shell

`flakeModules.default` is a flake-parts module for [numtide devshell](https://github.com/numtide/devshell). A repository that imports it and lists its languages gets, in its dev shell, `worktrees` as a command in the menu's `worktrees` category, and, each time the shell loads, the variables `worktrees env --languages <the list>` prints for the worktree it loads in. So every worktree of the repository shares the same `CARGO_BUILD_BUILD_DIR` and has its own `CARGO_TARGET_DIR`, both outside it, and a new worktree is provisioned the first time its dev shell loads.

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    devshell.url = "github:numtide/devshell";
    nix-worktrees.url = "github:nhooey/nix-worktrees";
  };

  outputs = inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      imports = [
        inputs.devshell.flakeModule # or agent-skill-flake's devshellSkills, which brings it
        inputs.nix-worktrees.flakeModules.default
      ];
      perSystem.worktrees.languages = [ "rust" "python" ];
    };
}
```

| Option                         | Default                                | Means                                                                                         |
| ------------------------------ | -------------------------------------- | --------------------------------------------------------------------------------------------- |
| `perSystem.worktrees.languages` | `null`                                 | the languages in the table above, aliases included; `null` lets `env` detect them from the worktree's files |
| `perSystem.worktrees.package`   | nix-worktrees' `packages.<system>.worktrees` | the program the shell runs and offers                                                   |
| `perSystem.worktrees.devshell`  | `"default"`                            | which of `devshells` to set up                                                                |
| `perSystem.worktrees.diskFloorWarning` | `true`                          | warn on standard error, when the shell loads, that its root's volume is below a [disk floor](#the-disk-floor-hook) |
| `perSystem.worktrees.hooksPlugin` | nix-worktrees' `packages.<system>.hooks-plugin` | the plugin whose `scripts/disk-floor.sh --warn` makes that check                   |

The module sets nothing itself: the shell's startup evals what `env` prints, so a variable already set when the shell loads is left alone, outside git nothing is set, and when `env` fails its reason goes to standard error and the shell loads without the variables. The disk floor check is the plugin's hook script run with `--warn`: it only warns, since a shell that won't start can't be used to clean up. The repository must import numtide devshell's flake module itself, since importing it twice declares `devshells` twice. A `nix build` or `nix flake check` runs in the sandbox with none of these variables and shares work through the store instead. `tests/dev-shell/` is a dev shell built this way, for Rust and Python, and `checks.dev-shell` loads it in a repository and a second worktree of it.

## In a Home Manager configuration

`homeManagerModules.default` sets up `worktrees` for a user, in every repository, flake or not: the program on the `PATH`, the environment loaded on entering a worktree, an hourly sweep, and the disk floors agents' builds stop at.

```nix
{
  imports = [ inputs.nix-worktrees.homeManagerModules.default ];
  programs.worktrees = {
    enable = true;
    cloneRoots = [ "~/git/github/nhooey" ];
    direnv.enable = true;
    fish.enable = true;
    sweep.enable = true;
  };
}
```

| Option                       | Default                                      | Means                                                                                                                                                                                  |
| ---------------------------- | -------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `enable`                     | `false`                                      | puts `worktrees` on the `PATH` and writes `config.json` and `floor` under `~/.config/worktrees/`                                                                                       |
| `package`                    | nix-worktrees' `packages.<system>.worktrees` | the program installed and run by the loaders and the sweep                                                                                                                            |
| `cloneRoots`                 | none: it must be set                         | the folders `--all` looks under, written to `config.json` as `roots`                                                                                                                   |
| `direnv.enable`              | `false`                                      | `use_worktrees` in `~/.config/direnv/lib/worktrees.sh`, which evals `worktrees env`; an `.envrc` calls it as `use worktrees`, and any arguments go to `env`, such as `--languages rust` |
| `fish.enable`                | `false`                                      | a function on `PWD` in `~/.config/fish/conf.d/worktrees.fish` that sources `worktrees env --shell fish` in interactive shells when the git root changes                               |
| `sweep.enable`               | `false`                                      | `worktrees sweep --all` every `sweep.interval`: a launchd agent on macOS, a systemd user timer on Linux, appending to `~/.local/state/worktrees/sweep.log`                             |
| `sweep.apply`                | `false`                                      | adds `--apply` to the scheduled sweep, and is written to `config.json` as `sweep.apply` for the plugin's sweep                                                                         |
| `sweep.interval`             | `3600`                                       | seconds between sweeps                                                                                                                                                                 |
| `sweep.cargoCapGB`           | `60`                                         | written to `config.json` as `cargo_cap_gb`                                                                                                                                             |
| `floor.buildGB`              | `70`                                         | written to `floor` as `WORKTREES_FLOOR_BUILD_GB`                                                                                                                                       |
| `floor.cleanupOnlyGB`        | `35`                                         | written to `floor` as `WORKTREES_FLOOR_CLEANUP_ONLY_GB`; must not be above `floor.buildGB`                                                                                             |

The floors are whole numbers so the plugin's shell hook can compare them without a parser. Leaving git in fish sets and unsets nothing; moving to another worktree replaces only the variables `env` set itself. In a repository whose dev shell also imports `flakeModules.default`, both loaders run `env` and print the same values, so the second changes nothing. `checks.home-manager-module` evaluates a configuration with every option on for Linux and macOS, with and without `sweep.apply`, compares the floor file, the settings and the sweep's command line with the options, and on the system it runs on builds the files and loads the direnv function and the fish hook in a Rust repository.

## In Claude Code sessions

`plugin/` is the Claude Code plugin `nix-worktrees-hooks`, built as `packages.<system>.hooks-plugin` and installed through agent-plugins-nixpkgs and managed settings. Its session hooks call `worktrees` in every session, interactive or headless, and each does nothing and prints nothing when `worktrees` isn't on the `PATH`; its disk floor hook needs no program.

| Event          | Does                                                                                                                                                                                                                            |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PreToolUse` on `Bash` | denies a build below the build floor, and everything but reading and cleaning up below the cleanup floor: [the disk floor hook](#the-disk-floor-hook)                                                                  |
| `SessionStart` | appends `command -v worktrees >/dev/null 2>&1 && eval "$(worktrees env </dev/null)"` to `$CLAUDE_ENV_FILE`; starts `worktrees sweep --json` for the session's repository, with `--apply` when `config.json` has `sweep.apply` true |
| `SubagentStop` | `worktrees clean <path> --apply` on the subagent's worktree, `.claude/worktrees/agent-<agent_id>` under the event's checkout or the main checkout, once `git worktree list` lists it and it is unlocked, or after 2 seconds        |
| `SessionEnd`   | `worktrees clean <path> --apply` on the session's own worktree, when its `cwd` is one and not the main checkout                                                                                                                   |

- **The environment line** runs before every Bash command, from the directory the command starts in, so a subagent in its own worktree builds into its own `CARGO_TARGET_DIR`.
- **What the sweep kept:** each session's sweep saves its report under `~/.local/state/worktrees/session-sweep/`. The next session in the repository reads it and, at most once a day, puts one line into the session's context, such as "3 idle unmerged worktrees in this repository hold 4G".
- **Never blocking:** the sweep and the cleans run detached, in their own process group, from `/`, so they outlive Claude Code, which gives a `SessionEnd` hook 1.5 seconds and kills a headless run's async hooks when it ends. A clean that refuses, as it does while the stopping session is still inside, is retried for up to 120 seconds.
- **Always exit 0:** a failure goes to `~/.local/state/worktrees/hooks.log`, never to the session.
- **jq and git** are store paths in the built plugin. Run from this checkout, as with `claude --plugin-dir plugin`, the scripts take them from the `PATH`.

`checks.hooks-plugin` runs shellcheck over the built plugin's scripts, `tests/plugin/hooks.bats`, with a stub `worktrees`, over temporary repositories, and `tests/plugin/disk-floor.bats`, with a stub `df`.

### The disk floor hook

`scripts/disk-floor.sh` runs before every Bash command an agent runs, and compares the free space on the command's working directory, from `df -Pk`, with [the disk floors](#the-disk-floors).

| Free space            | The hook                                                                                                      |
| --------------------- | ------------------------------------------------------------------------------------------------------------- |
| at or above the build floor (70 GB) | allows everything                                                                               |
| below the build floor | denies a command whose pipeline or list holds a build, any command matching a line of `plugin/build-launchers.txt` |
| below the cleanup floor (35 GB) | denies every command except `git`, `df`, `du`, `ls`, `cat`, `rm`, `cd` and `worktrees`             |

- **The denial** says how much is free and names the floor, then "Run `worktrees sweep --apply` and `worktrees status --all`, or ask the user.", so the agent's next step is cleanup.
- **`build-launchers.txt`** holds one extended regular expression per line, such as `cargo`, `nix (build|flake check|develop|run|shell)` and `(npm|pnpm|yarn|bun) (install|ci|run|build|test)`. Each command of the line is matched after its leading `VAR=value` assignments, its wrappers (`env`, `time`, `nice`, `nohup`, `exec`, `command`, `sudo`) and its first word's directory, so `FOO=1 /usr/bin/make` is `make`. Below the cleanup floor the list doesn't matter.
- **Fast:** a POSIX shell script run by dash, starting only df above the floors, and jq and awk as well below one. Over 100 calls on an Apple silicon Mac at a load average of 10 to 12, it took 7 to 10 ms each above the floors and 20 to 45 ms below the build floor, nearly all of it starting processes. `tests/plugin/disk-floor.bats` fails above 10 ms a call, or `DISK_FLOOR_MAX_US`; `checks.hooks-plugin` sets that to 50 ms, since the other checks build beside it.
- **Fails open:** when the event, `df`, the floor file or a floor can't be read, it allows the command and writes why to `~/.local/state/worktrees/hooks.log`.
- **The dev shell** runs the same script as `disk-floor.sh --warn <root>`, which warns and exits 0.
- **Hook modes, later:** the denial is decided in one function, `denial_reason`, so that a later change can put it behind agent-plugins-nixpkgs' `hook_mode` helper (`off`, `remind`, `deny`) once that helper is packaged. Until then the hook always denies.
- **Not the Nix daemon:** builds the daemon runs for others aren't stopped here; Nix's own `min-free` collection is the backstop there.

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

`status`, [the disk floor hook](#the-disk-floor-hook) and the dev shell's warning compare the free space with two floors, read from `WORKTREES_FLOOR_BUILD_GB` and `WORKTREES_FLOOR_CLEANUP_ONLY_GB`, else from the same `KEY=value` lines in `${XDG_CONFIG_HOME:-~/.config}/worktrees/floor`, else 70 and 35 GB, of 10^9 bytes.

## Logs

- `~/.local/state/worktrees/clean.log`: every path `clean` deletes, before it goes.
- `~/.local/state/worktrees/sweep.log`: each sweep's verdicts, removals, orphans, the Cargo cap, and once a day per repository the unmerged worktrees; the Home Manager module's scheduled sweep appends its report here too.
- `~/.local/state/worktrees/hooks.log`: what the Claude Code plugin's hooks couldn't do, one line each.

## Development

Enter the dev shell with `nix develop` (or direnv). Then:

- `worktrees`: the program, from this checkout.
- `tests`: the core's and the commands' tests, `python3 -m unittest`; without Nix, `python3 -m unittest discover --start-directory worktrees/tests --top-level-directory .`.
- `check`: `nix flake check`: formatting and lints (treefmt with nixfmt and ruff), the package build, the tests, `checks.dev-shell`, the dev shell module's test, `checks.home-manager-module`, the Home Manager module's, and `checks.hooks-plugin`, the plugin's.
- `bats tests/plugin/hooks.bats`: the plugin's hook tests alone, against `plugin/`, with `bats` and `jq` on the `PATH`.

The program is pure functions in `worktrees/core.py`, a thin shell around git, `ps`, `lsof`, `df`, `du` and the file system in `worktrees/system.py` and `worktrees/repo.py`, and the commands in `worktrees/cli.py`. The command tests build temporary git repositories with `HOME` and the XDG folders inside them; nothing is removed outside them.

## Licence

Apache-2.0; see `LICENSE`.
