#!/usr/bin/env bash
# The built files of a Home Manager configuration with programs.worktrees
# fully on: the floor file and settings as written, then the direnv function
# and the fish hook loading a Rust repository's variables with the real
# program.
#
# Usage: check.sh <home-files> <check.fish> <scratch directory>
set -euo pipefail

files=$1
fish_check=$2
scratch=$3

fail() {
	echo "check.sh: $*" >&2
	exit 1
}

config=$files/.config
want_floor=$'WORKTREES_FLOOR_BUILD_GB=80\nWORKTREES_FLOOR_CLEANUP_ONLY_GB=40'
[[ $(<"$config/worktrees/floor") == "$want_floor" ]] ||
	fail "the floor file is $(<"$config/worktrees/floor")"
grep -q '"cargo_cap_gb":50' "$config/worktrees/config.json" ||
	fail "config.json lacks the Cargo cap: $(<"$config/worktrees/config.json")"
bash -n "$config/direnv/lib/worktrees.sh"
fish --no-execute "$config/fish/conf.d/worktrees.fish"

# A Rust repository, committed, with a second worktree, under a HOME of its own.
export HOME=$scratch/home
unset XDG_CONFIG_HOME XDG_CACHE_HOME XDG_STATE_HOME CARGO_TARGET_DIR WORKTREE_BUILDS_WT WORKTREE_BUILDS_SET
mkdir -p "$HOME" "$scratch/repo"
repo=$(cd "$scratch/repo" && pwd -P)
git -C "$repo" init --quiet --initial-branch=main
touch "$repo/Cargo.toml"
git -C "$repo" add Cargo.toml
git -C "$repo" -c user.name=t -c user.email=t@example.com commit --quiet --message init
git -C "$repo" worktree add --quiet -b second "$repo.worktrees/second"
second=$(cd "$repo.worktrees/second" && pwd -P)

# direnv: `use worktrees` in an .envrc calls use_worktrees.
(
	# shellcheck source=/dev/null
	source "$config/direnv/lib/worktrees.sh"
	cd "$second"
	use_worktrees
	[[ ${CARGO_TARGET_DIR:-} == "$HOME/.cache/worktree-builds/"*/target ]] ||
		fail "use_worktrees set CARGO_TARGET_DIR to '${CARGO_TARGET_DIR:-}'"
)

fish --no-config --interactive \
	--command "source $(printf '%q' "$fish_check") \$argv" \
	"$config/fish/conf.d/worktrees.fish" "$repo" "$second" ||
	fail "the fish hook check failed"

echo "check.sh: the floor file, settings, direnv function and fish hook are right"
