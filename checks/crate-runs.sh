# shellcheck shell=bash
# The body of the `crate-runs` check in flake.nix. The derivation's
# attributes arrive as environment variables: `program` is the Rust binary.

export HOME=$TMPDIR/home
mkdir -p "$HOME"
"${program:?}" --version | grep -q '^[a-z-]* [0-9]'
"${program:?}" --help > /dev/null
git init --quiet repo
git -C repo -c user.name=Check -c user.email=check@example.invalid commit --quiet --allow-empty -m init
git -C repo worktree add --quiet -b feature ../feature
cd feature || exit
"${program:?}" env --json > exports.json
grep -q '"WORKTREE_BUILDS_WT"' exports.json
"${program:?}" clean --dry-run --json . > clean.json
grep -q '"would": true' clean.json
touch "${out:?}"
