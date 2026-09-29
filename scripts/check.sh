#!/usr/bin/env bash
# Keep the local checks and CI on the same commands.
set -euo pipefail

scope="${1:-all}"
case "$scope" in
  rust|raycast|all) ;;
  *) echo "Usage: scripts/check.sh [rust|raycast|all]" >&2; exit 2 ;;
esac
if [[ $# -gt 1 ]]; then
  echo "Usage: scripts/check.sh [rust|raycast|all]" >&2
  exit 2
fi

cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$scope" != raycast ]]; then
  command -v cargo >/dev/null || { echo "Install Rust and Cargo before running Rust checks." >&2; exit 1; }
  cargo fmt --version >/dev/null || { echo "Install rustfmt: rustup component add rustfmt" >&2; exit 1; }
  cargo clippy --version >/dev/null || { echo "Install Clippy: rustup component add clippy" >&2; exit 1; }
  cargo nextest --version >/dev/null || { echo "Install cargo-nextest before running Rust checks." >&2; exit 1; }
fi
if [[ "$scope" != rust ]]; then
  command -v npm >/dev/null || { echo "Install Node.js and npm before running Raycast checks." >&2; exit 1; }
  if [[ ! -d raycast/ms-todo/node_modules ]]; then
    echo "Install Raycast dependencies first: cd raycast/ms-todo && npm ci" >&2
    exit 1
  fi
fi

if [[ "$scope" != raycast ]]; then
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo nextest run --workspace --locked
fi
if [[ "$scope" != rust ]]; then
  cd raycast/ms-todo
  npm run typecheck
  npm test
  npm run lint
  npm run build
fi
