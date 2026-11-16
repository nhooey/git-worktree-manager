# shellcheck shell=bash
# What the plugin's three hook scripts share: reading the event, the log,
# running the program under a deadline, and detaching from Claude Code.
#
# Sourced by each hook after it has checked that `worktrees` is on the PATH.
# Written for bash 3.2, macOS's /bin/bash: no associative arrays, no mapfile.
#
# Settings for tests, all optional:
#   WORKTREES_HOOK_FOREGROUND=1  run the work in the hook's own process
#   WORKTREES_HOOK_TIMEOUT       seconds a clean may take in all (120)
#   WORKTREES_HOOK_UNLOCK_WAIT   seconds SubagentStop waits for the unlock (2)

# The package replaces these two with store paths.
WT_JQ=jq
WT_GIT=git

hook_name=${hook_name:-hook}
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/worktrees"

# One line in ~/.local/state/worktrees/hooks.log; never fails the hook.
log() {
  mkdir -p "$state_dir" 2>/dev/null || return 0
  printf '%s %s: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$hook_name" "$*" \
    >>"$state_dir/hooks.log" 2>/dev/null || true
}

# The event's JSON, read once from stdin.
event=
read_event() {
  event=$(cat 2>/dev/null) || event=
}

# One string field of the event, empty when missing or when jq can't read it.
event_field() {
  # shellcheck disable=SC2016 # a jq program, not shell
  printf '%s' "$event" | "$WT_JQ" -r --arg k "$1" '.[$k] // empty | strings' 2>/dev/null || true
}

# A directory's physical path, or nothing when it doesn't exist.
physical() {
  (cd -P -- "$1" 2>/dev/null && pwd -P)
}

# The worktree paths `git worktree list --porcelain` prints, main checkout first.
listed_worktrees() {
  "$WT_GIT" -C "$1" worktree list --porcelain 2>/dev/null | sed -n 's/^worktree //p'
}

# The listed path of the worktree at $2 in the repository at $1, compared by
# physical path; nothing when git doesn't list it.
listed_as() {
  local want listed
  want=$(physical "$2") || return 1
  [ -n "$want" ] || return 1
  while IFS= read -r listed; do
    if [ "$(physical "$listed")" = "$want" ]; then
      printf '%s\n' "$listed"
      return 0
    fi
  done <<EOF
$(listed_worktrees "$1")
EOF
  return 1
}

# Whether git lists the worktree at listed path $2 as locked.
is_locked() {
  "$WT_GIT" -C "$1" worktree list --porcelain 2>/dev/null |
    awk -v p="$2" '
      /^worktree / { mine = (substr($0, 10) == p) }
      mine && /^locked/ { found = 1 }
      END { exit !found }'
}

# Run "$@" until the absolute $SECONDS deadline $1, its output in $run_output.
# Returns the command's status, or 124 when the deadline killed it.
run_output=
run_until() {
  local deadline=$1 out pid status
  shift
  out=$(mktemp "${TMPDIR:-/tmp}/worktrees-hook.XXXXXX") || return 1
  "$@" >"$out" 2>&1 </dev/null &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      kill "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      run_output=$(tail -n 5 "$out" | tr '\n' ' ')
      rm -f "$out"
      return 124
    fi
    sleep 0.1
  done
  wait "$pid"
  status=$?
  run_output=$(tail -n 5 "$out" | tr '\n' ' ')
  rm -f "$out"
  return "$status"
}

# `worktrees clean <path> --apply`, retried while it refuses (exit 3), as it
# does while the stopping session or agent is still inside, until the
# deadline. Every outcome but a clean one goes to the log.
clean_worktree() {
  local path=$1 deadline delay=1 status
  deadline=$((SECONDS + ${WORKTREES_HOOK_TIMEOUT:-120}))
  while :; do
    run_until "$deadline" worktrees clean "$path" --apply
    status=$?
    case $status in
    0) return 0 ;;
    3) ;;
    124)
      log "worktrees clean $path timed out: $run_output"
      return 0
      ;;
    *)
      log "worktrees clean $path failed ($status): $run_output"
      return 0
      ;;
    esac
    if [ $((SECONDS + delay)) -ge "$deadline" ]; then
      log "worktrees clean $path kept it: $run_output"
      return 0
    fi
    sleep "$delay"
    delay=$((delay * 2))
  done
}

# Run "$@" apart from Claude Code, which kills a SessionEnd hook after 1.5 s
# and a headless run's async hooks at teardown: in the background, in its own
# process group (job control), with no terminal, from /, so that its own
# working directory never keeps a worktree in use.
detach() {
  if [ -n "${WORKTREES_HOOK_FOREGROUND:-}" ]; then
    (cd / && "$@")
    return 0
  fi
  set -m
  (
    trap '' HUP
    cd / && "$@"
  ) </dev/null >/dev/null 2>&1 &
  set +m
  return 0
}
