#!/usr/bin/env bash
# SubagentStop: clean the build output of a subagent's own worktree.
#
# The event names no worktree, so the path is derived: Claude Code makes an
# agent's worktree `agent-<agent_id>` under the checkout's .claude/worktrees/.
# The hook looks there under the event's checkout and under the main
# checkout, and acts only on a path `git worktree list` lists; a subagent
# without a worktree of its own matches nothing. It waits up to about
# 2 seconds for Claude Code to unlock the worktree, then runs
# `worktrees clean <path> --apply` detached, which refuses a worktree a live
# Claude Code process still holds or has tracked changes in.
#
# Does nothing and prints nothing when `worktrees` isn't on the PATH, and
# always exits 0; failures go to ~/.local/state/worktrees/hooks.log.

command -v worktrees >/dev/null 2>&1 || exit 0

hook_name=SubagentStop
# shellcheck source=hook-lib.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/hook-lib.sh"

read_event
agent_id=$(event_field agent_id)
cwd=$(event_field cwd)
[ -n "$cwd" ] || cwd=$PWD

# An id with anything but these characters isn't one Claude Code makes.
case $agent_id in
'' | *[!A-Za-z0-9_-]*) exit 0 ;;
esac

top=$("$WT_GIT" -C "$cwd" rev-parse --show-toplevel 2>/dev/null) || exit 0
main=$(listed_worktrees "$cwd" | head -n 1)
name=".claude/worktrees/agent-$agent_id"

path=
for checkout in "$top" "$main"; do
  [ -n "$checkout" ] || continue
  if path=$(listed_as "$cwd" "$checkout/$name"); then
    break
  fi
  path=
done
# The event's cwd may be the agent's worktree itself.
if [ -z "$path" ] && [ "$(basename -- "$top")" = "agent-$agent_id" ]; then
  path=$(listed_as "$cwd" "$top") || path=
fi
[ -n "$path" ] || exit 0

# shellcheck disable=SC2329 # run by detach
stopped() {
  local tries
  tries=$((${WORKTREES_HOOK_UNLOCK_WAIT:-2} * 10))
  while [ "$tries" -gt 0 ] && is_locked "$main" "$path"; do
    sleep 0.1
    tries=$((tries - 1))
  done
  clean_worktree "$path"
}

detach stopped
exit 0
