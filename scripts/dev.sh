#!/usr/bin/env bash
# Thin cargo wrapper. Sources the user-local rustup environment
# (rustup installs cargo to ~/.cargo/bin, which isn't on $PATH by
# default) and forwards every arg to cargo.
#
# Usage:
#   ./scripts/dev.sh check                       # cargo check
#   ./scripts/dev.sh test -p barme-render-s3o    # cargo test ...
#   ./scripts/dev.sh clippy --workspace --all-targets -- -D warnings
#   ./scripts/dev.sh fmt --check
#
# Rationale: avoids `. "$HOME/.cargo/env" && cargo ...` boilerplate
# in every shell invocation.
set -euo pipefail

if [[ -f "${HOME}/.cargo/env" ]]; then
  # shellcheck disable=SC1091
  . "${HOME}/.cargo/env"
fi

exec cargo "$@"
