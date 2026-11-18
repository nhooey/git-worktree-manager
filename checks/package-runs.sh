# shellcheck shell=bash
# The body of the `package-runs` check in flake.nix. The derivation's
# attributes arrive as environment variables: `worktrees` is the packaged
# program.

export HOME=$TMPDIR/home
mkdir -p "$HOME"
"${worktrees:?}" --help > /dev/null
git init --quiet repo
cd repo || exit
"${worktrees:?}" sweep --dry-run --json > report.json
grep -q '"dry_run": true' report.json
touch "${out:?}"
