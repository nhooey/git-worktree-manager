# The fish hook in an interactive shell: entering a Rust repository with no
# dev shell sets CARGO_TARGET_DIR, entering a second worktree of it moves it
# to that worktree's, and leaving git leaves the variables as they were set.
#
# Usage: fish --interactive --command 'source check.fish $argv' <worktrees.fish> <repo> <second worktree>
set -l hook $argv[1]
set -l repo $argv[2]
set -l second $argv[3]

function fail
    echo "check.fish: $argv" >&2
    exit 1
end

source $hook

cd $repo
string match -q -- "$HOME/.cache/worktree-builds/*/target" "$CARGO_TARGET_DIR"
or fail "in the repository CARGO_TARGET_DIR is '$CARGO_TARGET_DIR'"
set -l in_repo $CARGO_TARGET_DIR

cd $second
string match -q -- "$HOME/.cache/worktree-builds/*/target" "$CARGO_TARGET_DIR"
or fail "in the second worktree CARGO_TARGET_DIR is '$CARGO_TARGET_DIR'"
test "$CARGO_TARGET_DIR" != "$in_repo"
or fail "the second worktree kept the repository's CARGO_TARGET_DIR, '$in_repo'"
set -l in_second $CARGO_TARGET_DIR

cd /
test "$CARGO_TARGET_DIR" = "$in_second"
or fail "leaving git changed CARGO_TARGET_DIR to '$CARGO_TARGET_DIR'"

echo "check.fish: the fish hook loads and moves the variables"
