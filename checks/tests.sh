# shellcheck shell=bash
# The body of the `tests` check in flake.nix. The derivation's attributes
# arrive as environment variables: `sourceTree` is this repository's tree.

cp -r "${sourceTree:?}" src
chmod -R u+w src
cd src || exit
patchShebangs bin
export HOME=$TMPDIR/home
mkdir -p "$HOME"
python3 -m unittest discover --start-directory worktrees/tests --top-level-directory . --verbose
touch "${out:?}"
