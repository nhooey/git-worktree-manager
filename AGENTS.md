# Working on this repository

nix-worktrees holds everything about agent worktrees: the `worktrees`
program, and later the flake module for dev shells, the Home Manager module
and the Claude Code plugin whose hooks clean and sweep worktrees and keep
agents' builds above a disk floor. `README.md` describes how they behave.

## Checks

There is no CI. Before committing, run `nix flake check`: formatting
(treefmt). Stage new files first, since Nix only sees tracked ones.

## Dev shell

Enter it with `nix develop` (or direnv); it installs the project's skills
into `.agents/skills/` from the `skills-devshell/` sub-flake. `menu` lists
its commands.
