#!/usr/bin/env bash
# SessionEnd: clean the build output of the session's own worktree.
#
# The session's cwd, when it is a worktree and not the main checkout (a
# session started with --worktree or EnterWorktree, or a headless run in a
# worktree of its own), gets `worktrees clean <path> --apply`, detached so
# that it outlives Claude Code, and retried while the clean refuses because
# the session's process is still inside, up to 120 seconds in all.
#
# Does nothing and prints nothing when `worktrees` isn't on the PATH, and
# always exits 0; failures go to ~/.local/state/worktrees/hooks.log.

command -v worktrees >/dev/null 2>&1 || exit 0

hook_name=SessionEnd
# shellcheck source=hook-lib.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/hook-lib.sh"

read_event
cwd=$(event_field cwd)
[ -n "$cwd" ] || cwd=$PWD

top=$("$WT_GIT" -C "$cwd" rev-parse --show-toplevel 2>/dev/null) || exit 0
main=$(listed_worktrees "$cwd" | head -n 1)
[ -n "$main" ] || exit 0
[ "$(physical "$top")" != "$(physical "$main")" ] || exit 0
path=$(listed_as "$cwd" "$top") || exit 0

detach clean_worktree "$path"
exit 0
