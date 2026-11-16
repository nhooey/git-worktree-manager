#!/usr/bin/env bats
# The plugin's hook scripts, run against a stub `worktrees` in temporary git
# repositories. No network, and no writes outside $BATS_TEST_TMPDIR.
#
# PLUGIN_ROOT names the plugin to test: the built package in `nix flake
# check`, plugin/ in this checkout otherwise.

setup() {
  PLUGIN_ROOT="${PLUGIN_ROOT:-$(cd "$BATS_TEST_DIRNAME/../../plugin" && pwd)}"
  T="$BATS_TEST_TMPDIR"

  export HOME="$T/home"
  export XDG_STATE_HOME="$HOME/.local/state"
  export XDG_CONFIG_HOME="$HOME/.config"
  mkdir -p "$HOME"
  export GIT_CONFIG_GLOBAL="$T/gitconfig" GIT_CONFIG_NOSYSTEM=1
  git config --global user.name test
  git config --global user.email test@example.com
  git config --global init.defaultBranch main

  export WORKTREES_HOOK_FOREGROUND=1
  export WORKTREES_HOOK_TIMEOUT=5
  HOOK_LOG="$XDG_STATE_HOME/worktrees/hooks.log"
  ENV_FILE="$T/claude-env-file.sh"

  # The PATH without any real `worktrees`, as the dev shell has one.
  BARE_PATH=
  local dir
  local IFS=:
  for dir in $PATH; do
    [ -x "$dir/worktrees" ] || BARE_PATH="${BARE_PATH:+$BARE_PATH:}$dir"
  done
  unset IFS

  STUB_DIR="$T/stub-bin"
  export STUB_LOG="$T/stub.log"
  mkdir -p "$STUB_DIR"
  cat >"$STUB_DIR/worktrees" <<'EOF'
#!/usr/bin/env bash
# Records each call as "<cwd> | <arguments>", and for clean whether git
# still lists a lock; exits 3 for the first $STUB_REFUSE calls, then
# $STUB_EXIT (0).
printf '%s | %s\n' "$(pwd -P)" "$*" >>"$STUB_LOG"
if [ "$1" = clean ] && git -C "$2" worktree list --porcelain | grep -q '^locked'; then
  echo "locked at clean" >>"$STUB_LOG"
fi
sleep "${STUB_SLEEP:-0}"
calls=$(grep -c ' | ' "$STUB_LOG")
if [ "$calls" -le "${STUB_REFUSE:-0}" ]; then
  echo "kept $2: in use by process 1"
  exit 3
fi
if [ "$1" = sweep ]; then
  cat "${STUB_SWEEP_JSON:-/dev/null}"
fi
[ "${STUB_EXIT:-0}" -eq 0 ] || echo "worktrees: stub failure" >&2
exit "${STUB_EXIT:-0}"
EOF
  chmod +x "$STUB_DIR/worktrees"
  STUB_PATH="$STUB_DIR:$BARE_PATH"

  # A repository with one commit and an agent's worktree.
  REPO="$T/repo"
  git init -q "$REPO"
  git -C "$REPO" commit -q --allow-empty -m start
  AGENT=abc123
  AGENT_WT="$REPO/.claude/worktrees/agent-$AGENT"
  git -C "$REPO" worktree add -q -b "worktree-agent-$AGENT" "$AGENT_WT"
  REPO=$(cd -P "$REPO" && pwd -P)
  AGENT_WT=$(cd -P "$AGENT_WT" && pwd -P)
}

hook() { # <script> <event JSON>: run a hook with the stub on the PATH
  PATH="$STUB_PATH" CLAUDE_ENV_FILE="$ENV_FILE" run bash "$PLUGIN_ROOT/scripts/$1" <<<"$2"
}

calls() { grep -c ' | ' "$STUB_LOG" 2>/dev/null || true; }

wait_for_calls() { # <count>: for the detached work
  local i
  for i in $(seq 100); do
    [ "$(calls)" -ge "$1" ] && return 0
    sleep 0.1
  done
  return 1
}

ENV_LINE='command -v worktrees >/dev/null 2>&1 && eval "$(worktrees env </dev/null)"'

# hooks.json ----------------------------------------------------------------------

@test "hooks.json: every hook runs one of the plugin's scripts, which parses as bash" {
  run jq -r '.hooks[][].hooks[] | .command' "$PLUGIN_ROOT/hooks/hooks.json"
  [ "$status" -eq 0 ]
  [ "${#lines[@]}" -eq 3 ]
  for command in "${lines[@]}"; do
    [[ $command =~ ^bash\ \"\$\{CLAUDE_PLUGIN_ROOT\}/scripts/([a-z-]+\.sh)\"$ ]]
    bash -n "$PLUGIN_ROOT/scripts/${BASH_REMATCH[1]}"
  done
  jq -e '.name and .version' "$PLUGIN_ROOT/.claude-plugin/plugin.json"
}

@test "hooks.json: SubagentStop and SessionEnd are async with a 120 s timeout" {
  jq -e '[.hooks.SubagentStop, .hooks.SessionEnd][][].hooks[] | .async == true and .timeout == 120' \
    "$PLUGIN_ROOT/hooks/hooks.json"
}

# No program on the PATH -------------------------------------------------------------

@test "no worktrees on the PATH: every hook exits 0, prints nothing and writes nothing" {
  for script in session-start.sh subagent-stop.sh session-end.sh; do
    PATH="$BARE_PATH" CLAUDE_ENV_FILE="$ENV_FILE" run bash "$PLUGIN_ROOT/scripts/$script" \
      <<<"{\"cwd\": \"$AGENT_WT\", \"agent_id\": \"$AGENT\"}"
    [ "$status" -eq 0 ]
    [ -z "$output" ]
  done
  [ ! -e "$ENV_FILE" ]
  [ ! -e "$STUB_LOG" ]
  [ ! -e "$XDG_STATE_HOME/worktrees" ]
}

# SessionStart -------------------------------------------------------------------------

@test "SessionStart: appends the environment line once across two sessions" {
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$(grep -cxF -- "$ENV_LINE" "$ENV_FILE")" -eq 1 ]
}

@test "SessionStart: the environment line loads worktrees env, and nothing without it" {
  printf '#!/bin/sh\necho export WT_LOADED=1\n' >"$STUB_DIR/worktrees"
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  run env PATH="$STUB_PATH" bash -c ". \"$ENV_FILE\"; echo \"\${WT_LOADED-unset}\""
  [ "$output" = 1 ]
  run env PATH="$BARE_PATH" bash -c ". \"$ENV_FILE\"; echo \"\${WT_LOADED-unset}\""
  [ "$status" -eq 0 ]
  [ "$output" = unset ]
}

@test "SessionStart: without CLAUDE_ENV_FILE, still sweeps and exits 0" {
  PATH="$STUB_PATH" run env -u CLAUDE_ENV_FILE bash "$PLUGIN_ROOT/scripts/session-start.sh" \
    <<<"{\"cwd\": \"$REPO\"}"
  [ "$status" -eq 0 ]
  grep -qxF "$REPO | sweep --json" "$STUB_LOG"
}

@test "SessionStart: sweeps the session's repository without --apply by default" {
  hook session-start.sh "{\"cwd\": \"$AGENT_WT\"}"
  [ "$status" -eq 0 ]
  [ "$(cat "$STUB_LOG")" = "$AGENT_WT | sweep --json" ]
}

@test "SessionStart: sweeps with --apply when sweep.apply is true" {
  mkdir -p "$XDG_CONFIG_HOME/worktrees"
  echo '{"roots":["~/git"],"sweep":{"apply":true}}' >"$XDG_CONFIG_HOME/worktrees/config.json"
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$(cat "$STUB_LOG")" = "$REPO | sweep --apply --json" ]
}

@test "SessionStart: sweeps without --apply when sweep.apply is false" {
  mkdir -p "$XDG_CONFIG_HOME/worktrees"
  echo '{"sweep": {"apply": false}}' >"$XDG_CONFIG_HOME/worktrees/config.json"
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$(cat "$STUB_LOG")" = "$REPO | sweep --json" ]
}

@test "SessionStart: outside git, only the environment line" {
  mkdir -p "$T/plain"
  hook session-start.sh "{\"cwd\": \"$T/plain\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  [ ! -e "$STUB_LOG" ]
  grep -qxF -- "$ENV_LINE" "$ENV_FILE"
}

@test "SessionStart: reports the last sweep's idle unmerged worktrees once a day" {
  export STUB_SWEEP_JSON="$T/sweep.json"
  cat >"$STUB_SWEEP_JSON" <<EOF
{"repositories": [{"repository": "$REPO", "unmerged": [
  {"worktree": "$AGENT_WT", "branch": "a", "size": 3221225472, "reasons": []},
  {"worktree": "$REPO/x", "branch": "b", "size": 1073741824, "reasons": []},
  {"worktree": "$REPO/y", "branch": "c", "size": null, "reasons": []}]}],
 "orphan_repositories": [], "applied": false}
EOF
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ -z "$output" ] # the first session's sweep only saves its report
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$output" = '3 idle unmerged worktrees in this repository hold 4G; `worktrees status` lists them.' ]
  hook session-start.sh "{\"cwd\": \"$AGENT_WT\"}"
  [ -z "$output" ] # the same repository, the same day
}

@test "SessionStart: reports nothing when the sweep kept no unmerged worktree" {
  export STUB_SWEEP_JSON="$T/sweep.json"
  echo '{"repositories": [{"repository": "r", "unmerged": []}]}' >"$STUB_SWEEP_JSON"
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ -z "$output" ]
}

@test "SessionStart: a failing sweep exits 0, prints nothing and logs a line" {
  export STUB_EXIT=1
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  grep -q 'SessionStart: worktrees sweep in .* failed (1): worktrees: stub failure' "$HOOK_LOG"
}

@test "SessionStart: the sweep runs detached, after the hook has returned" {
  unset WORKTREES_HOOK_FOREGROUND
  export STUB_SLEEP=3
  start=$SECONDS
  hook session-start.sh "{\"cwd\": \"$REPO\"}"
  [ "$status" -eq 0 ]
  [ $((SECONDS - start)) -lt 3 ]
  wait_for_calls 1
  [ "$(cat "$STUB_LOG")" = "$REPO | sweep --json" ]
}

# SubagentStop --------------------------------------------------------------------------

@test "SubagentStop: cleans the agent's derived worktree" {
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"$AGENT\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  [ "$(cat "$STUB_LOG")" = "/ | clean $AGENT_WT --apply" ]
}

@test "SubagentStop: matches from the agent's worktree and from another worktree" {
  hook subagent-stop.sh "{\"cwd\": \"$AGENT_WT\", \"agent_id\": \"$AGENT\"}"
  git -C "$REPO" worktree add -q -b other "$T/other"
  hook subagent-stop.sh "{\"cwd\": \"$T/other\", \"agent_id\": \"$AGENT\"}"
  [ "$(grep -cxF "/ | clean $AGENT_WT --apply" "$STUB_LOG")" -eq 2 ]
}

@test "SubagentStop: a subagent without a worktree of its own matches nothing" {
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"def456\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  [ ! -e "$STUB_LOG" ]
}

@test "SubagentStop: a folder named like the worktree that git doesn't list matches nothing" {
  mkdir -p "$REPO/.claude/worktrees/agent-def456"
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"def456\"}"
  [ ! -e "$STUB_LOG" ]
}

@test "SubagentStop: an agent id with a path in it matches nothing" {
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"../agent-$AGENT\"}"
  hook subagent-stop.sh "{\"cwd\": \"$REPO\"}"
  hook subagent-stop.sh 'not json'
  [ "$status" -eq 0 ]
  [ ! -e "$STUB_LOG" ]
}

@test "SubagentStop: waits for Claude Code to unlock the worktree" {
  git -C "$REPO" worktree lock --reason "claude agent agent-$AGENT (pid 1 start 0)" "$AGENT_WT"
  (sleep 0.5 && git -C "$REPO" worktree unlock "$AGENT_WT") &
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"$AGENT\"}"
  wait
  [ "$(cat "$STUB_LOG")" = "/ | clean $AGENT_WT --apply" ]
}

@test "SubagentStop: still locked after the wait, leaves the refusal to clean" {
  git -C "$REPO" worktree lock --reason "claude agent agent-$AGENT (pid 1 start 0)" "$AGENT_WT"
  # Refused at once and a second later; a third try wouldn't fit in 3 s.
  export WORKTREES_HOOK_UNLOCK_WAIT=1 WORKTREES_HOOK_TIMEOUT=3 STUB_REFUSE=9
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"$AGENT\"}"
  [ "$status" -eq 0 ]
  grep -qxF "locked at clean" "$STUB_LOG"
  grep -q "SubagentStop: worktrees clean $AGENT_WT kept it: kept" "$HOOK_LOG"
}

@test "SubagentStop: a failing clean exits 0 and logs a line" {
  export STUB_EXIT=1
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"$AGENT\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  grep -q "SubagentStop: worktrees clean $AGENT_WT failed (1): worktrees: stub failure" "$HOOK_LOG"
}

@test "SubagentStop: a clean past the timeout is stopped and logged" {
  export STUB_SLEEP=5 WORKTREES_HOOK_TIMEOUT=1
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"$AGENT\"}"
  [ "$status" -eq 0 ]
  grep -q "SubagentStop: worktrees clean $AGENT_WT timed out" "$HOOK_LOG"
}

@test "SubagentStop: the clean runs detached, after the hook has returned" {
  unset WORKTREES_HOOK_FOREGROUND
  hook subagent-stop.sh "{\"cwd\": \"$REPO\", \"agent_id\": \"$AGENT\"}"
  [ "$status" -eq 0 ]
  wait_for_calls 1
  [ "$(cat "$STUB_LOG")" = "/ | clean $AGENT_WT --apply" ]
}

# SessionEnd ---------------------------------------------------------------------------------

@test "SessionEnd: cleans the session's own worktree" {
  hook session-end.sh "{\"cwd\": \"$AGENT_WT\", \"reason\": \"other\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  [ "$(cat "$STUB_LOG")" = "/ | clean $AGENT_WT --apply" ]
}

@test "SessionEnd: cleans from a subfolder of the worktree" {
  mkdir -p "$AGENT_WT/src/deep"
  hook session-end.sh "{\"cwd\": \"$AGENT_WT/src/deep\"}"
  [ "$(cat "$STUB_LOG")" = "/ | clean $AGENT_WT --apply" ]
}

@test "SessionEnd: never the main checkout, and nothing outside git" {
  hook session-end.sh "{\"cwd\": \"$REPO\"}"
  [ "$status" -eq 0 ]
  mkdir -p "$T/plain"
  hook session-end.sh "{\"cwd\": \"$T/plain\"}"
  [ "$status" -eq 0 ]
  [ ! -e "$STUB_LOG" ]
}

@test "SessionEnd: retries while clean refuses, as while the session is still inside" {
  export STUB_REFUSE=2
  hook session-end.sh "{\"cwd\": \"$AGENT_WT\"}"
  [ "$status" -eq 0 ]
  [ "$(calls)" -eq 3 ]
  [ ! -e "$HOOK_LOG" ]
}

@test "SessionEnd: a failing clean exits 0 and logs a line" {
  export STUB_EXIT=1
  hook session-end.sh "{\"cwd\": \"$AGENT_WT\"}"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  grep -q "SessionEnd: worktrees clean $AGENT_WT failed (1): worktrees: stub failure" "$HOOK_LOG"
}
