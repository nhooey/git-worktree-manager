# Working on this repository

This repository holds one Rust crate, `git-worktree-manager`, which builds one binary of that name. It plans and carries out the end of git worktrees and branches, archives what would be lost to a local backup, keeps worktrees' build output outside them, and is git's `reference-transaction` hook. `README.md` describes how it behaves; `flake.nix` packages it and holds the checks.

## The code

`src/lib.rs` lists the modules; `src/main.rs` only hands the arguments and the real system to `cli::run`.

- **The values:** `src/model/` holds what every part passes along: `RepositoryFacts` (what git and the system said, typed, with no text left to parse), `Settings`, `Plan`, `Outcomes`, the `Report` that `status`, `sweep`, `finish`, `archive write` and `archive verify` print, and the log's `Event`; every other command's JSON is a struct of its own, in its module under `commands/`, in `disk.rs`, or, for `rules`, in `rules/catalogue.rs`. `src/model/report/node-example.json` is the report's example.
- **The pure modules** decide, over values read beforehand:
  - `rules/`: `rules::plan` and `rules::finish`, the branch and worktree rules, survival, attention, and the catalogue `rules` prints;
  - `report.rs`, and `render/` with `render::text` and `render::markdown` over a `Report`;
  - `keep.rs` (`keep::split`), `landed.rs` (the content-landed comparison), `lock.rs`, `time.rs`, `names.rs` and `rescue.rs` (`rescue::candidates`, the only reader of the log);
  - the pure parts of `backup.rs`, `archive.rs`, `build_root.rs`, `disk.rs` and `hook.rs`.
- **The reading edge:** `targets.rs` names the repositories a command runs over, `locate.rs` finds each (upward from the folder a command runs in, or a manifest entry at its own path only), `locate::open` is the one place a located repository is opened, at its own path, where no `GIT_*` variable chooses its repository, worktree, objects or config, and `read/` reads it through gitoxide (`gix`) into `RepositoryFacts`. They write nothing. A read `gix` can't do runs `git` through the system edge, and `README.md` names each.
- **The acting edge:** `sweep/` (`act.rs` carries a plan out in the sweep's order), the writing parts of `archive.rs`, and `commands/`, one module per command, each joining a read, a pure decision and the acts. Every change goes through `git`, another program such as `direnv`, or the file system, behind the system edge.
- **The system edge:** the `System` trait in `src/system.rs`, in front of `git`, `ps`, the clock, the environment, the file system and free space. Reads take `&self` and acts `&mut self`, so a signature says whether it can change anything. `system/real.rs` is the system the binary runs on; `system/fake.rs` answers from tables and records every act, for unit tests.
- **The command line:** `cli.rs`, with `clap`; each command's options are declared beside it in `commands/`.

Unit tests sit in their module, or in a `tests.rs` beside it. `tests/` holds the integration tests, one file per command or part, which run the built binary or the library over real repositories in the sandbox, `tests/sandbox/mod.rs`: a temporary directory with `HOME` and the XDG folders inside it, and repositories with a bare remote. `tests/fixtures/report/` holds the renderers' fixture reports with their expected text, Markdown and JSON.

## Style

- `#![forbid(unsafe_code)]` in `lib.rs` and `main.rs`.
- No `static mut`, no interior mutability and no shared mutable state, but for the `AtomicBool` in `keep.rs` that gitoxide's directory walk takes as its interrupt flag, which nothing sets.
- Pure functions over immutable values decide; one edge reads the facts and one carries the plan out.
- No `unwrap`, `expect` or `panic!` outside tests, but for an `expect` on a value that can't be absent, as in `backup.rs`'s free name suffix and `lock.rs`'s constant patterns; every other failure is a `Failure` with a code, printed by `commands::failed`.
- Long options only.

## The contract

The commands, their options, the JSON report and the exit statuses are a contract with the callers: github-config, the hooks, the dev shell module and agent sessions run the binary and read its `--format json` output and exit statuses. Change them only with those callers in mind, and raise `schema` for a change that breaks a reader.

Never run a sweep without `--dry-run` against a real checkout while developing: test every removal in the sandbox.

## Checks

There is no CI. Before committing, run:

- `nix fmt`: treefmt, with nixfmt and rustfmt;
- `cargo test`, and `cargo clippy --all-targets -- --deny warnings`;
- `nix flake check`: the package build, a run of the package (`testers.testVersion`), `crate-tests`, `crate-clippy` and formatting.

Stage new files first: Nix sees only tracked ones. The hook's timing budgets are ignored tests, run by name in a release build: `cargo test --release --test hook -- --ignored --nocapture`.

## Dev shell

Enter it with `nix develop` (or direnv). It holds `cargo`, `rustc`, `clippy`, `rustfmt`, `git`, `ps`, and `fish` for the `env --shell fish` test, and installs the project's skills into `.agents/skills/` from the `skills-devshell/` sub-flake. `menu` lists its commands.
