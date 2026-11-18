# shellcheck shell=bash
# The body of the `package-runs` check in flake.nix. The derivation's
# attributes arrive as environment variables: `worktrees` is the packaged
# program.

export HOME=$TMPDIR/home
mkdir -p "$HOME"
"${worktrees:?}" --help > /dev/null
touch "${out:?}"
