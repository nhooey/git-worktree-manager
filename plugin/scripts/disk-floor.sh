#!/bin/sh
# PreToolUse on Bash: refuse a build below the build floor, and everything
# but reading and cleaning up below the cleanup floor.
#
# Usage:
#   disk-floor.sh              the hook: the event's JSON on standard input;
#                              prints a denial, or nothing to allow
#   disk-floor.sh --warn DIR   the dev shell's check: a warning on standard
#                              error when DIR's volume is below a floor;
#                              always exits 0
#
# Free space is `df -Pk` on the command's working directory, the event's
# `cwd`. The floors, in GB of 10^9 bytes, are WORKTREES_FLOOR_BUILD_GB and
# WORKTREES_FLOOR_CLEANUP_ONLY_GB, else the same `KEY=value` lines in
# ${XDG_CONFIG_HOME:-~/.config}/worktrees/floor, which the Home Manager
# module writes, else 70 and 35.
#
# Below the build floor a command is denied when any command of its pipeline
# or list matches a line of ../build-launchers.txt. Below the cleanup floor
# it is denied unless every command is git, df, du, ls, cat, rm, cd or
# worktrees.
#
# Fails open: when the event, df, the floor file or a floor's value can't be
# read, the command is allowed and the reason goes to
# ~/.local/state/worktrees/hooks.log. Always exits 0.
#
# POSIX sh, since it runs before every Bash command, and every process it
# starts costs a few milliseconds: above the floors it starts only df; below
# one, jq for the command line and awk to match it. The package runs it with
# dash, which starts in half the time of macOS's /bin/sh.
#
# Settings for tests, all optional:
#   WORKTREES_FLOOR_DF   the df to run, in place of the package's own

# The package replaces jq with a store path, and df with the system's on
# macOS, which starts faster than coreutils', and coreutils' elsewhere.
WT_JQ=jq
WT_DF=${WORKTREES_FLOOR_DF:-df}

case $0 in
*/*) here=${0%/*} ;;
*) here=. ;;
esac
launchers="$here/../build-launchers.txt"
nl='
'

log() {
  dir="${XDG_STATE_HOME:-$HOME/.local/state}/worktrees"
  mkdir -p "$dir" 2>/dev/null || return 0
  printf '%s DiskFloor: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" \
    >>"$dir/hooks.log" 2>/dev/null || true
}

# A floor is a whole number of GB, at most 9 digits so the byte comparison
# stays inside 64-bit shell arithmetic.
is_gb() {
  case $1 in
  '' | *[!0-9]* | ??????????*) return 1 ;;
  esac
}

# Sets build_gb and cleanup_gb; returns 1, after logging why, when a value
# can't be read.
read_floors() {
  build_gb=
  cleanup_gb=
  file="${XDG_CONFIG_HOME:-$HOME/.config}/worktrees/floor"
  if [ -e "$file" ]; then
    if [ ! -r "$file" ]; then
      log "can't read $file; allowing"
      return 1
    fi
    while IFS='=' read -r key value || [ -n "$key" ]; do
      case $key in
      WORKTREES_FLOOR_BUILD_GB) build_gb=$value ;;
      WORKTREES_FLOOR_CLEANUP_ONLY_GB) cleanup_gb=$value ;;
      esac
    done <"$file"
  fi
  [ -z "${WORKTREES_FLOOR_BUILD_GB:-}" ] || build_gb=$WORKTREES_FLOOR_BUILD_GB
  [ -z "${WORKTREES_FLOOR_CLEANUP_ONLY_GB:-}" ] || cleanup_gb=$WORKTREES_FLOOR_CLEANUP_ONLY_GB
  build_gb=${build_gb:-70}
  cleanup_gb=${cleanup_gb:-35}
  if ! is_gb "$build_gb" || ! is_gb "$cleanup_gb"; then
    log "floors '$build_gb' and '$cleanup_gb' aren't whole numbers of GB; allowing"
    return 1
  fi
}

# Sets free_kb, and free_gb in whole GB, for the volume holding $1; returns
# 1, after logging why, when df can't tell. The available column is the one
# before the capacity, the first field ending in %, since the file system's
# name and the mount point may both hold spaces.
read_free() {
  free_kb=
  out=$("$WT_DF" -Pk -- "$1" 2>&1) || {
    log "df -Pk $1 failed: $out; allowing"
    return 1
  }
  line=${out#*"$nl"}
  line=${line%%"$nl"*}
  prev=
  set -f
  # shellcheck disable=SC2086 # split into fields on purpose
  for field in $line; do
    case $field in
    *%)
      free_kb=$prev
      break
      ;;
    esac
    prev=$field
  done
  set +f
  case $free_kb in
  '' | *[!0-9]*)
    log "can't read the free space in df -Pk $1: $out; allowing"
    return 1
    ;;
  esac
  free_gb=$((free_kb * 1024 / 1000000000))
}

# Sets state to the floor the free space is below: `build`, `cleanup` or
# nothing.
floor_state() {
  state=
  if [ $((free_kb * 1024)) -lt $((cleanup_gb * 1000000000)) ]; then
    state=cleanup
  elif [ $((free_kb * 1024)) -lt $((build_gb * 1000000000)) ]; then
    state=build
  fi
}

# The first command of the Bash command line on standard input that the
# floor $1 refuses, escaped for a JSON string, or nothing. Commands are split
# at |, ||, &&, ;, &, newlines, parentheses, braces and backquotes outside
# quotes; each loses its leading variable assignments, its wrappers and its
# first word's directory before it is matched.
refused_command() {
  awk -v floor="$1" -v launchers="$launchers" '
    BEGIN {
      if (floor == "build")
        while ((getline line < launchers) > 0)
          if (line !~ /^[[:space:]]*(#|$)/) pats[npats++] = line
      split("git df du ls cat rm cd worktrees", names, " ")
      for (i in names) cleanup[names[i]] = 1
    }
    { text = text $0 "\n" }
    END {
      n = length(text); seg = ""; q = ""
      for (i = 1; i <= n; i++) {
        c = substr(text, i, 1)
        if (q != "") {
          if (c == q) q = ""
          else if (c == "\\" && q == "\"") { seg = seg c; i++; c = substr(text, i, 1) }
          seg = seg c
          continue
        }
        if (c == "\\") { seg = seg c substr(text, i + 1, 1); i++; continue }
        if (c == "\"" || c == "\047") { q = c; seg = seg c; continue }
        p = substr(text, i - 1, 1); x = substr(text, i + 1, 1)
        if (c == "&" && (p == ">" || p == "<" || x == ">")) { seg = seg c; continue }
        if (index("|;&\n(){}`", c)) { if (check(seg)) exit; seg = ""; continue }
        seg = seg c
      }
      check(seg)
    }
    function check(s,    w, k) {
      sub(/^[[:space:]]+/, "", s)
      while (s ~ /^([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*|env|time|nice|nohup|exec|command|sudo|!)([[:space:]]|$)/)
        { sub(/^[^[:space:]]*[[:space:]]*/, "", s) }
      sub(/[[:space:]]+$/, "", s)
      if (s == "" || s == "$") return 0
      sub(/^[^[:space:]]*\//, "", s)
      if (floor == "cleanup") {
        w = s; sub(/[[:space:]].*/, "", w)
        if (w in cleanup) return 0
        return refuse(s)
      }
      for (k = 0; k < npats; k++)
        if (s ~ ("^(" pats[k] ")([[:space:]]|$)")) return refuse(s)
      return 0
    }
    function refuse(s) {
      gsub(/\\/, "\\\\", s); gsub(/"/, "\\\"", s); gsub(/\t/, "\\t", s)
      gsub(/[[:cntrl:]]/, "", s)
      print s
      return 1
    }'
}

# shellcheck disable=SC2016 # backquotes for Markdown, not command substitution
advice='Run `worktrees sweep --apply` and `worktrees status --all`'

# The one place the hook decides to deny: prints the reason, ready to go in a
# JSON string, or nothing to allow. A later change puts its result behind
# the plugin's hook mode (`hook_mode disk-floor`: off, remind or deny).
denial_reason() { # <state> <command line>
  refused=$(printf '%s\n' "$2" | refused_command "$1")
  [ -n "$refused" ] || return 0
  case $1 in
  cleanup)
    # shellcheck disable=SC2016 # backquotes for Markdown
    printf 'Only %s GB free, below the %s GB cleanup floor, where only git, df, du, ls, cat, rm, cd and worktrees run (`%s` is refused). %s, or ask the user.' \
      "$free_gb" "$cleanup_gb" "$refused" "$advice"
    ;;
  build)
    printf 'Only %s GB free, below the %s GB build floor. %s, or ask the user.' \
      "$free_gb" "$build_gb" "$advice"
    ;;
  esac
}

if [ "${1:-}" = --warn ]; then
  read_floors && read_free "${2:-.}" || exit 0
  floor_state
  case $state in
  cleanup)
    printf 'worktrees: only %s GB free on %s, below the %s GB cleanup floor: agents may only read and clean up. %s.\n' \
      "$free_gb" "${2:-.}" "$cleanup_gb" "$advice" >&2
    ;;
  build)
    printf 'worktrees: only %s GB free on %s, below the %s GB build floor: agents may not build. %s.\n' \
      "$free_gb" "${2:-.}" "$build_gb" "$advice" >&2
    ;;
  esac
  exit 0
fi

event=
while IFS= read -r line || [ -n "$line" ]; do
  event="$event$line$nl"
done

# The cwd, read without jq when it holds no escape, as Claude Code writes it.
cwd=
rest=${event#*\"cwd\"}
if [ "$rest" != "$event" ]; then
  rest=${rest#"${rest%%[!	 :]*}"}
  case $rest in
  \"*)
    rest=${rest#\"}
    cwd=${rest%%\"*}
    ;;
  esac
fi
case $cwd in
*\\*) cwd=$(printf '%s' "$event" | "$WT_JQ" -r '.cwd // empty' 2>/dev/null) ;;
esac
[ -n "$cwd" ] || cwd=$PWD

read_floors && read_free "$cwd" || exit 0
floor_state
[ -n "$state" ] || exit 0

command=$(printf '%s' "$event" | "$WT_JQ" -r '.tool_input.command // empty' 2>&1) || {
  log "can't read the event: $command; allowing"
  exit 0
}
[ -n "$command" ] || exit 0
reason=$(denial_reason "$state" "$command")
[ -n "$reason" ] || exit 0

printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"%s"}}\n' \
  "$reason"
exit 0
