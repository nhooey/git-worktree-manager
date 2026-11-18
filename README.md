# nix-worktrees

Keeps git worktrees' build output outside them and shared.

It holds one program, `worktrees`. Git hooks, dev shells, agent sessions and people all call it the same way, and it runs without Nix.

## Install and run

- **With Nix:** `nix run github:nhooey/nix-worktrees -- env`, or the `worktrees` package (`packages.<system>.default`, also `.worktrees`).
- **Without Nix:** `bin/worktrees` from a clone, or `python3 -m worktrees` with the clone on `PYTHONPATH`. It needs Python 3.9 or later and nothing outside the standard library. `pip install .` gives the Python commands, without `bin/worktrees`.
- **It runs** `git` 2.31 or later.

## The commands

Every command takes `--help` and long options only.

| Command | Does |
| --- | --- |
| `worktrees env`, `provision`, `clean`, `sessions` | build environment, provisioning, cleaning a worktree's ignored build output, and a checkout's Claude Code sessions |

- **Exit status:** 0 done; 1 a repository failed; 2 bad usage; 3 `clean` refused.

## The build root

`worktrees env` prints export lines that put each worktree's build output under `${XDG_CACHE_HOME:-~/.cache}/worktree-builds/<repo>-<hash8>/`, outside the worktree. Caches that can be shared go under `shared/`; per-worktree output goes under `wt/<wt-id>/`. Run it as `eval "$(worktrees env)"`. It detects the languages from the files at the worktree's root, or takes them from `--languages`.

## Development

`nix develop` enters the dev shell, with `worktrees` and `tests` as commands. `nix flake check` runs:
- treefmt with nixfmt, ruff and shellcheck;
- the package build, and a run of the package;
- every test.

Without Nix, run `python3 -m unittest discover --start-directory worktrees/tests --top-level-directory .`.
