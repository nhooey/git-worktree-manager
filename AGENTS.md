# Working on this repository

nix-worktrees holds everything about git worktrees: the `worktrees`
program, and later the flake module for dev shells, the Home Manager module
and the Claude Code plugin whose hooks start and report its sweep.
`README.md` describes how they behave.

## The program

- `bin/worktrees`: the one entry point, POSIX `sh`. It runs
  `python3 -m worktrees`.
- `worktrees/rules.py`: every decision of the sweep, as pure functions over
  immutable values (NamedTuples, tuples, frozensets), as
  `python-hygiene-functional-style` asks at its standard-library tier.
- `worktrees/core.py`: the pure core of `env`, `provision` and `clean`.
- `worktrees/gather.py`: reads git, `ps` and the file system, read-only,
  into `rules.RepoFacts`.
- `worktrees/sweep.py`: acts on `rules.plan` in the sweep's order, logs
  each action and builds the report.
- `worktrees/system.py`: runs commands and touches files. Every git command
  runs with `--no-optional-locks`.
- `worktrees/cli.py`: the commands and their options, long options only.
- `worktrees/defaults.json`: the keep patterns and every setting's default.
- `worktrees/tests/`: `test_core.py` and `test_rules.py` test the pure
  code directly; the others run the commands over temporary repositories
  with a bare remote, with `HOME` and the XDG folders inside them.

It must run on Python 3.9, macOS's `/usr/bin/python3`: no `match`, no
`tomllib`, no `X | Y` outside annotations, no `dataclass(slots=True)`.

The commands are a contract: github-config, the dev shell module, the hooks
and the headless runner call them and read their `--json` output and exit
codes. Change them only with those callers in mind.

Never run a sweep without `--dry-run` against a real checkout while
developing: test every removal in temporary repositories.

## Checks

There is no CI. Before committing, run `nix flake check`: formatting and
lints (treefmt with nixfmt, ruff and shellcheck), the package build, a run
of the package, and every test. Stage new files first, since Nix only sees
tracked ones. Without Nix, `python3 -m unittest discover --start-directory
worktrees/tests --top-level-directory .` runs the tests; `tests` does the
same in the dev shell.

`checks/` holds the shell bodies of the checks that run commands, each
read by `builtins.readFile`, with the Nix values it needs passed as
derivation attributes, which arrive as environment variables.

## Dev shell

Enter it with `nix develop` (or direnv); it installs the project's skills
into `.agents/skills/` from the `skills-devshell/` sub-flake. `menu` lists
its commands.
