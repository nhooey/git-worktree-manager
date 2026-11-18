# shellcheck shell=bash
# The body of the `crate-runs` check in flake.nix. The derivation's
# attributes arrive as environment variables: `program` is the Rust binary.

set -euo pipefail
"${program:?}" --version | grep -q '^[a-z-]* [0-9]'
"${program:?}" --help | grep -q '^Usage: '
touch "${out:?}"
