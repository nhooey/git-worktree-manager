# Working on this repository

nix-worktrees holds everything about agent worktrees: the `worktrees`
program, the flake module for dev shells (`flake-module.nix`, exported as
`flakeModules.default` and tested by `tests/dev-shell/`), the Home Manager
module (`home-manager/`, exported as `homeManagerModules.default` and
tested by `tests/home-manager/`), and later the Claude Code plugin whose hooks clean and sweep worktrees and keep
agents' builds above a disk floor. `README.md` describes how they behave.

## The program

- `worktrees/core.py`: every decision, as pure functions over immutable
  values (NamedTuples, tuples, frozensets), as `python-hygiene-functional-style`
  asks at its standard-library tier. No I/O.
- `worktrees/system.py` and `worktrees/repo.py`: the thin shell around git,
  `ps`, `lsof`, `df`, `du` and the file system.
- `worktrees/cli.py`: the commands and their options, long options only.
- `worktrees/defaults.json`: every setting with its default.
- `worktrees/tests/`: `test_core.py` tests the core directly;
  `test_commands.py` runs `python3 -m worktrees` over temporary git
  repositories, with `HOME` and the XDG folders inside them.

It must run on Python 3.9, macOS's `/usr/bin/python3`: no `match`, no
`tomllib`, no `X | Y` outside annotations, no `dataclass(slots=True)`.

The commands are a contract: the dev shell module, the Home Manager module,
the plugin's hooks and the headless runner call them and read their `--json`
output and exit codes. Change them only with those callers in mind.

Never run `--apply` against a real checkout while developing: test every
removal in temporary repositories.

## Checks

There is no CI. Before committing, run `nix flake check`: formatting and
lints (treefmt with nixfmt and ruff), the package build and the tests.
Stage new files first, since Nix only sees tracked ones. `tests` runs the
tests alone.

## Dev shell

Enter it with `nix develop` (or direnv); it installs the project's skills
into `.agents/skills/` from the `skills-devshell/` sub-flake. `menu` lists
its commands.
