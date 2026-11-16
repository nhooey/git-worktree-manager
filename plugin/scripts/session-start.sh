#!/usr/bin/env bash
# SessionStart: load the worktree environment into every Bash command,
# report the repository's idle unmerged worktrees once a day, and sweep the
# repository in the background.
#
# - Appends one line to $CLAUDE_ENV_FILE, which Claude Code runs before
#   every Bash command, so each command gets `worktrees env` for the
#   directory it starts in, a subagent's own worktree included.
# - Prints one line of context, at most once a day per repository, when the
#   last session sweep of this repository found idle unmerged worktrees.
# - Starts `worktrees sweep --json`, with --apply when the configuration's
#   sweep.apply is true, detached, and saves its report for the next session.
#
# Does nothing and prints nothing when `worktrees` isn't on the PATH, and
# always exits 0; failures go to ~/.local/state/worktrees/hooks.log.

command -v worktrees >/dev/null 2>&1 || exit 0

hook_name=SessionStart
# shellcheck source=hook-lib.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/hook-lib.sh"

read_event

# The environment ---------------------------------------------------------------

# shellcheck disable=SC2016 # literal text for CLAUDE_ENV_FILE, expanded at each command
line='command -v worktrees >/dev/null 2>&1 && eval "$(worktrees env </dev/null)"'
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  if [ ! -f "$CLAUDE_ENV_FILE" ] || ! grep -qxF -- "$line" "$CLAUDE_ENV_FILE"; then
    printf '%s\n' "$line" >>"$CLAUDE_ENV_FILE" || log "could not write $CLAUDE_ENV_FILE"
  fi
fi

# The repository ----------------------------------------------------------------

cwd=$(event_field cwd)
[ -n "$cwd" ] || cwd=$PWD
common=$("$WT_GIT" -C "$cwd" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || exit 0
[ -n "$common" ] || exit 0
key=$(printf '%s' "$common" | cksum | awk '{print $1}')
reports="$state_dir/session-sweep"
report="$reports/$key.json"

# What the last sweep kept ---------------------------------------------------------

# "3 idle unmerged worktrees in this repository hold 4G", sizes formatted as
# the program's human_bytes does; nothing when there are none.
summary() {
  # shellcheck disable=SC2016 # a jq program, not shell
  "$WT_JQ" -r '
    def human: [., 0]
      | until(.[0] < 1024 or .[1] == 4; [.[0] / 1024, .[1] + 1])
      | if .[1] == 0 then "\(.[0])B"
        else "\(.[0] + 0.5 | floor)\(["B", "K", "M", "G", "T"][.[1]])" end;
    [.repositories[]? | .unmerged[]?] as $u
    | select($u | length > 0)
    | "\($u | length) idle unmerged worktree\(if ($u | length) == 1 then "" else "s" end)"
      + " in this repository hold\(if ($u | length) == 1 then "s" else "" end)"
      + " \([$u[].size // 0] | add | human); `worktrees status` lists them."
  ' "$1" 2>/dev/null
}

today=$(date +%Y-%m-%d)
stamp="$reports/$key.reported"
if [ -f "$report" ] && [ "$(cat "$stamp" 2>/dev/null)" != "$today" ]; then
  said=$(summary "$report")
  if [ -n "$said" ]; then
    printf '%s\n' "$said"
    printf '%s\n' "$today" >"$stamp" 2>/dev/null || log "could not write $stamp"
  fi
fi

# The sweep ---------------------------------------------------------------------

config="${XDG_CONFIG_HOME:-$HOME/.config}/worktrees/config.json"
apply=
if [ -f "$config" ] && "$WT_JQ" -e '.sweep.apply == true' "$config" >/dev/null 2>&1; then
  apply=--apply
fi

# shellcheck disable=SC2329 # run by detach
sweep() {
  local out status
  mkdir -p "$reports" || {
    log "could not create $reports"
    return 0
  }
  out=$(mktemp "$reports/.$key.XXXXXX") || return 0
  # No deadline of its own: the sweep bounds its fetch by fetch_timeout_seconds.
  (
    cd "$cwd" || exit 1
    # shellcheck disable=SC2086 # $apply is empty or one word
    worktrees sweep $apply --json >"$out" 2>"$out.err" </dev/null
  )
  status=$?
  if [ "$status" -ne 0 ]; then
    log "worktrees sweep${apply:+ $apply} in $cwd failed ($status): $(tail -n 5 "$out.err" | tr '\n' ' ')"
  fi
  if "$WT_JQ" -e '.repositories' "$out" >/dev/null 2>&1; then
    mv -f "$out" "$report"
  else
    rm -f "$out"
  fi
  rm -f "$out.err"
}

detach sweep
exit 0
