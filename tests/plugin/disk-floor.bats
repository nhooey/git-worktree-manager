#!/usr/bin/env bats
# The disk floor hook, scripts/disk-floor.sh, with `df` stubbed through
# WORKTREES_FLOOR_DF: the stub prints a `df -Pk` table whose available column
# is $STUB_DF_GB GB, or fails when $STUB_DF_FAIL is set. The run-time test
# uses the hook's own df. No writes outside $BATS_TEST_TMPDIR.
#
# PLUGIN_ROOT names the plugin to test: the built package in `nix flake
# check`, plugin/ in this checkout otherwise. DISK_FLOOR_SH names the shell
# to run the hook with, `sh` by default and dash in `nix flake check`, as
# the package's hooks.json does. DISK_FLOOR_MAX_US is the run-time limit,
# 10000.

setup() {
  PLUGIN_ROOT="${PLUGIN_ROOT:-$(cd "$BATS_TEST_DIRNAME/../../plugin" && pwd)}"
  HOOK="$PLUGIN_ROOT/scripts/disk-floor.sh"
  T="$BATS_TEST_TMPDIR"

  export HOME="$T/home"
  export XDG_STATE_HOME="$HOME/.local/state"
  export XDG_CONFIG_HOME="$HOME/.config"
  mkdir -p "$HOME"
  unset WORKTREES_FLOOR_BUILD_GB WORKTREES_FLOOR_CLEANUP_ONLY_GB
  HOOK_LOG="$XDG_STATE_HOME/worktrees/hooks.log"
  FLOOR_FILE="$XDG_CONFIG_HOME/worktrees/floor"

  STUB_DIR="$T/stub-bin"
  mkdir -p "$STUB_DIR"
  cat >"$STUB_DIR/df" <<'EOF'
#!/bin/sh
# `df -Pk <dir>` for a volume with $STUB_DF_GB GB (10^9 bytes) available,
# on a mount point with a space in it.
if [ -n "${STUB_DF_FAIL:-}" ]; then
  echo "df: $3: No such file or directory" >&2
  exit 1
fi
avail=$((STUB_DF_GB * 1000000000 / 1024))
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/disk3s5 971350180 1000 $avail 50% /System/Volumes/My Data"
EOF
  chmod +x "$STUB_DIR/df"
  export WORKTREES_FLOOR_DF="$STUB_DIR/df"
  export STUB_DF_GB=100
}

hook() { # <command line>: run the hook on a Bash event for it
  local event
  event=$(jq -cn --arg cwd "$T" --arg c "$1" '{hook_event_name: "PreToolUse", tool_name: "Bash", cwd: $cwd, tool_input: {command: $c}}')
  run "${DISK_FLOOR_SH:-sh}" "$HOOK" <<<"$event"
}

allowed() {
  [ "$status" -eq 0 ]
  [ -z "$output" ]
}

denied() { # <floor word in the reason>
  [ "$status" -eq 0 ]
  printf '%s' "$output" | jq -e --arg w "$1" '
    .hookSpecificOutput.hookEventName == "PreToolUse"
    and .hookSpecificOutput.permissionDecision == "deny"
    and (.hookSpecificOutput.permissionDecisionReason | contains("below the " + $w)
      and contains("Run `worktrees sweep --apply` and `worktrees status --all`, or ask the user."))'
}

# The node's Check table -------------------------------------------------------------

@test "cargo build: allowed at 80 GB" {
  STUB_DF_GB=80 hook 'cargo build'
  allowed
}

@test "cargo build: denied at 60 GB, with the node's reason" {
  STUB_DF_GB=60 hook 'cargo build'
  denied "70 GB build floor"
  [ "$(printf '%s' "$output" | jq -r .hookSpecificOutput.permissionDecisionReason)" = \
    'Only 60 GB free, below the 70 GB build floor. Run `worktrees sweep --apply` and `worktrees status --all`, or ask the user.' ]
}

@test "ls: allowed at 30 GB" {
  STUB_DF_GB=30 hook 'ls -la'
  allowed
}

@test "python3 x.py: denied at 30 GB" {
  STUB_DF_GB=30 hook 'python3 x.py'
  denied "35 GB cleanup floor"
}

@test "the floors come from the floor file" {
  mkdir -p "$(dirname "$FLOOR_FILE")"
  printf 'WORKTREES_FLOOR_BUILD_GB=90\nWORKTREES_FLOOR_CLEANUP_ONLY_GB=50\n' >"$FLOOR_FILE"
  STUB_DF_GB=80 hook 'cargo build'
  denied "90 GB build floor"
  STUB_DF_GB=45 hook 'python3 x.py'
  denied "50 GB cleanup floor"
  STUB_DF_GB=95 hook 'cargo build'
  allowed
}

@test "the variables override the floor file" {
  mkdir -p "$(dirname "$FLOOR_FILE")"
  printf 'WORKTREES_FLOOR_BUILD_GB=90\nWORKTREES_FLOOR_CLEANUP_ONLY_GB=50\n' >"$FLOOR_FILE"
  WORKTREES_FLOOR_BUILD_GB=40 WORKTREES_FLOOR_CLEANUP_ONLY_GB=20 STUB_DF_GB=45 hook 'cargo build'
  allowed
  WORKTREES_FLOOR_BUILD_GB=40 WORKTREES_FLOOR_CLEANUP_ONLY_GB=20 STUB_DF_GB=30 hook 'cargo build'
  denied "40 GB build floor"
  WORKTREES_FLOOR_BUILD_GB=40 WORKTREES_FLOOR_CLEANUP_ONLY_GB=20 STUB_DF_GB=15 hook 'python3 x.py'
  denied "20 GB cleanup floor"
}

@test "an unreadable df allows the command and logs why" {
  STUB_DF_FAIL=1 STUB_DF_GB=1 hook 'cargo build'
  allowed
  grep -q 'DiskFloor: df -Pk .* failed: .*; allowing' "$HOOK_LOG"
}

# Around it ---------------------------------------------------------------------------

@test "a floor that isn't a whole number allows the command and logs why" {
  WORKTREES_FLOOR_BUILD_GB=lots STUB_DF_GB=1 hook 'cargo build'
  allowed
  grep -q "DiskFloor: floors 'lots' and '35' aren't whole numbers of GB; allowing" "$HOOK_LOG"
}

@test "an event that isn't JSON allows the command and logs why" {
  STUB_DF_GB=1 run "${DISK_FLOOR_SH:-sh}" "$HOOK" <<<'not json'
  allowed
  grep -q "DiskFloor: can't read the event" "$HOOK_LOG"
}

@test "just below the build floor counts as below it" {
  STUB_DF_GB=69 hook 'make'
  denied "70 GB build floor"
}

@test "below the build floor: only commands that start a build are denied" {
  for command in 'python3 x.py' 'git status' 'nix eval .#foo' 'nixfmt build' 'cargo-ish' \
    'npm view react' 'echo "make; cargo build"' 'git commit -m "a | make"'; do
    STUB_DF_GB=50 hook "$command"
    allowed
  done
}

@test "below the build floor: a launcher anywhere in a pipeline or list is denied" {
  for command in 'cargo test --workspace' 'git pull && make -j4' 'ls; nix flake check' \
    'FOO=1 BAR=2 nix build .#x' 'env RUST_LOG=debug cargo run' '/usr/bin/make install' \
    './gradlew assemble' 'time go test ./...' 'echo hi | pnpm install' 'cd sub && uv sync' \
    '(cd sub; ninja)' 'sudo docker build .' 'make 2>&1 | tee build.log' 'echo $(swift build)' \
    'tofu init' 'cmake --build build' 'pip install foo' 'bb test' $'true\nnix-build'; do
    STUB_DF_GB=50 hook "$command"
    denied "70 GB build floor"
  done
}

@test "below the cleanup floor: reading and cleaning up are allowed" {
  for command in 'git worktree list' 'df -h .' 'du -sh target' 'cat floor' 'rm -r target' \
    'cd ~/git/x && worktrees sweep --apply' 'worktrees status --all 2>&1 | cat' \
    'git commit -m "free space; rm build | cat"'; do
    STUB_DF_GB=20 hook "$command"
    allowed
  done
}

@test "below the cleanup floor: anything else is denied, naming the command" {
  for command in 'cargo build' 'git status && python3 x.py' 'ls | grep x' 'echo $(date)'; do
    STUB_DF_GB=20 hook "$command"
    denied "35 GB cleanup floor"
  done
  STUB_DF_GB=20 hook 'ls | grep x'
  printf '%s' "$output" | jq -e '.hookSpecificOutput.permissionDecisionReason | contains("(`grep x` is refused)")'
}

@test "a command line with quotes and backslashes still gives valid JSON" {
  STUB_DF_GB=20 hook 'printf "%s\n" "a \"b\"" \\ c'
  denied "35 GB cleanup floor"
}

@test "an event with no command is allowed" {
  STUB_DF_GB=1 run "${DISK_FLOOR_SH:-sh}" "$HOOK" <<<'{"cwd": "/", "tool_input": {}}'
  allowed
}

@test "df is run on the event's cwd" {
  cat >"$STUB_DIR/df" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >"$T/df-args"
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/x 1 1 999999999 1% /"
EOF
  hook 'cargo build'
  allowed
  [ "$(cat "$T/df-args")" = "-Pk -- $T" ]
}

# The dev shell's warning --------------------------------------------------------------

@test "--warn: a warning on standard error below a floor, never a failure" {
  STUB_DF_GB=60 run "${DISK_FLOOR_SH:-sh}" "$HOOK" --warn "$T"
  [ "$status" -eq 0 ]
  [[ $output == "worktrees: only 60 GB free on $T, below the 70 GB build floor: agents may not build."* ]]
  STUB_DF_GB=20 run "${DISK_FLOOR_SH:-sh}" "$HOOK" --warn "$T"
  [ "$status" -eq 0 ]
  [[ $output == *"below the 35 GB cleanup floor"* ]]
  STUB_DF_GB=80 run "${DISK_FLOOR_SH:-sh}" "$HOOK" --warn "$T"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
  STUB_DF_FAIL=1 run "${DISK_FLOOR_SH:-sh}" "$HOOK" --warn "$T"
  [ "$status" -eq 0 ]
  [ -z "$output" ]
}

# Run time --------------------------------------------------------------------------------

per_call_us() { # <build floor>: microseconds per call over 100, with the real df
  local event start end i
  event=$(jq -cn --arg cwd "$T" '{cwd: $cwd, tool_input: {command: "git status && cargo build"}}')
  start=$(date +%s%N)
  for ((i = 0; i < 100; i++)); do
    WORKTREES_FLOOR_DF= WORKTREES_FLOOR_BUILD_GB=$1 WORKTREES_FLOOR_CLEANUP_ONLY_GB=1 \
      "${DISK_FLOOR_SH:-sh}" "$HOOK" <<<"$event" >/dev/null
  done
  end=$(date +%s%N)
  echo $(((end - start) / 100000))
}

# The time every Bash command pays is the one above the floors; below one,
# the hook starts jq and awk as well, and that time is reported alongside.
@test "100 calls above the floors take under 10 ms each" {
  [[ $(date +%s%N) =~ ^[0-9]+$ ]] || skip "date has no %N"
  above=$(per_call_us 1)
  below=$(per_call_us 999999999)
  echo "# ${above} us per call above the floors, ${below} us below the build floor" >&3
  [ "$above" -lt "${DISK_FLOOR_MAX_US:-10000}" ]
}
