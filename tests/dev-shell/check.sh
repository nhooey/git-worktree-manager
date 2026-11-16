#!/usr/bin/env bash
# Checks a dev shell built from tests/dev-shell/flake.nix, which imports
# flakeModules.default with languages rust and python.
#
# Usage: check.sh <entrypoint> <scratch folder>
#
#   <entrypoint>      the dev shell's entrypoint script, as in
#                     $(nix build --print-out-paths <devshell package>)/entrypoint
#   <scratch folder>  an empty folder; HOME, the XDG folders and the test
#                     repositories go in it, so the build root does too
#
# It builds a git repository with a second worktree and checks, by running a
# command in the dev shell in each: the menu lists `worktrees`; both get the
# Rust and Python variables, the same CARGO_BUILD_BUILD_DIR and different
# CARGO_TARGET_DIRs, all outside both worktrees; an unlisted language's
# variable is not set; a variable set beforehand keeps its value; the second
# worktree is provisioned once its dev shell has loaded; below the build
# floor it warns and still loads, and above the floors it doesn't warn; and
# outside git nothing is set. `nix flake check` runs it as `checks.dev-shell`.
set -euo pipefail

entry=$1
scratch=$(cd "$2" && pwd -P)

export HOME="$scratch/home"
export XDG_CACHE_HOME="$HOME/.cache" XDG_CONFIG_HOME="$HOME/.config" XDG_STATE_HOME="$HOME/.local/state"
mkdir -p "$HOME"
unset CARGO_BUILD_BUILD_DIR CARGO_TARGET_DIR PYTHONPYCACHEPREFIX UV_PROJECT_ENVIRONMENT \
  RUFF_CACHE_DIR MYPY_CACHE_DIR TF_DATA_DIR WORKTREE_BUILDS_ROOT WORKTREE_BUILDS_WT WORKTREE_BUILDS_SET
builds="$XDG_CACHE_HOME/worktree-builds/"

failures=0
fail() {
  echo "FAIL: $*" >&2
  failures=$((failures + 1))
}
pass() { echo "ok: $*"; }

# The value of variable $2 in the dev shell loaded in folder $1.
in_shell() {
  (cd "$1" && "$entry" --prj-root "$1" bash -c 'printf %s "${!1-}"' _ "$2")
}

main="$scratch/repo"
second="$scratch/second"
git init -q "$main"
git -C "$main" -c user.name=test -c user.email=test@example.com commit -q --allow-empty -m init
git -C "$main" worktree add -q -b second "$second"

if (cd "$main" && "$entry" --prj-root "$main" menu) | grep -q '^ *worktrees'; then
  pass "the menu lists worktrees"
else
  fail "the menu doesn't list worktrees"
fi

for name in WORKTREE_BUILDS_ROOT WORKTREE_BUILDS_WT CARGO_BUILD_BUILD_DIR CARGO_TARGET_DIR \
  PYTHONPYCACHEPREFIX UV_PROJECT_ENVIRONMENT; do
  for wt in "$main" "$second"; do
    value=$(in_shell "$wt" "$name")
    case $value in
      "$builds"*) pass "$name in $wt is $value" ;;
      *) fail "$name in $wt is '$value', not under $builds" ;;
    esac
  done
done

shared_main=$(in_shell "$main" CARGO_BUILD_BUILD_DIR)
shared_second=$(in_shell "$second" CARGO_BUILD_BUILD_DIR)
if [[ -n $shared_main && $shared_main == "$shared_second" ]]; then
  pass "CARGO_BUILD_BUILD_DIR is shared"
else
  fail "CARGO_BUILD_BUILD_DIR differs: '$shared_main' and '$shared_second'"
fi

target_main=$(in_shell "$main" CARGO_TARGET_DIR)
target_second=$(in_shell "$second" CARGO_TARGET_DIR)
if [[ -n $target_main && -n $target_second && $target_main != "$target_second" ]]; then
  pass "CARGO_TARGET_DIR differs"
else
  fail "CARGO_TARGET_DIR is '$target_main' and '$target_second'"
fi

if [[ -z $(in_shell "$second" TF_DATA_DIR) ]]; then
  pass "TF_DATA_DIR, for a language not listed, is not set"
else
  fail "TF_DATA_DIR is set though terraform isn't listed"
fi

preset=$(CARGO_TARGET_DIR="$scratch/preset" in_shell "$second" CARGO_TARGET_DIR)
if [[ $preset == "$scratch/preset" ]]; then
  pass "a CARGO_TARGET_DIR set beforehand keeps its value"
else
  fail "a CARGO_TARGET_DIR set beforehand became '$preset'"
fi

if [[ -e "$(git -C "$second" rev-parse --absolute-git-dir)/worktrees-provisioned" ]]; then
  pass "the second worktree is provisioned"
else
  fail "the second worktree has no provisioning marker"
fi

unset WORKTREES_FLOOR_BUILD_GB WORKTREES_FLOOR_CLEANUP_ONLY_GB
low=$(cd "$main" && WORKTREES_FLOOR_BUILD_GB=999999999 WORKTREES_FLOOR_CLEANUP_ONLY_GB=1 \
  "$entry" --prj-root "$main" bash -c 'echo loaded' 2>&1) || fail "the dev shell failed below the build floor"
if [[ $low == *"below the 999999999 GB build floor"* && $low == *loaded* ]]; then
  pass "below the build floor, the dev shell warns and loads"
else
  fail "below the build floor, the dev shell printed: $low"
fi

high=$(cd "$main" && WORKTREES_FLOOR_BUILD_GB=1 WORKTREES_FLOOR_CLEANUP_ONLY_GB=1 \
  "$entry" --prj-root "$main" bash -c 'echo loaded' 2>&1)
if [[ $high != *floor* && $high == *loaded* ]]; then
  pass "above the floors, the dev shell doesn't warn"
else
  fail "above the floors, the dev shell printed: $high"
fi

mkdir "$scratch/plain"
if [[ -z $(in_shell "$scratch/plain" CARGO_TARGET_DIR) ]]; then
  pass "outside git, CARGO_TARGET_DIR is not set"
else
  fail "outside git, CARGO_TARGET_DIR is set"
fi

if ((failures > 0)); then
  echo "$failures checks failed" >&2
  exit 1
fi
