#!/usr/bin/env bash
# Run ms-todo against a fake Microsoft Graph with made-up data, for demos
# and recordings. Nothing here touches your real ms-todo data or Microsoft:
# HOME and the XDG directories point into a throwaway directory, the
# instance is `demo`, the sign-in is a fake token, and a debug build sends
# every Graph request to demo/fake-graph.
#
#   demo/run.sh              a shell where `mst` is the demo
#   demo/run.sh -- CMD ARGS  run CMD in that environment, then clean up
#
# DEMO_GRAPH_PORT picks the fake's port (47812 by default).

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
port="${DEMO_GRAPH_PORT:-47812}"

if [[ "${1:-}" == "--" ]]; then
  shift
fi

# Debug builds only: release builds ignore MS_TODO_GRAPH_URL on purpose.
cargo build --quiet --manifest-path "$repo/Cargo.toml" --locked \
  -p ms-todo -p ms-todo-demo-graph --bins
target="$(cargo metadata --manifest-path "$repo/Cargo.toml" --format-version 1 --no-deps |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')/debug"

# Under /tmp: macOS caps a socket path at 104 bytes, and $TMPDIR plus
# "Library/Application Support/…" is over.
demo_home="$(mktemp -d /tmp/mst-demo.XXXXXX)"
graph_pid=""

cleanup() {
  # This instance's daemon only: the demo's own symlink and environment.
  if [[ -x "$demo_home/bin/ms-todo" ]]; then
    "$demo_home/bin/ms-todo" daemon stop >/dev/null 2>&1 || true
  fi
  if [[ -n "$graph_pid" ]]; then
    kill "$graph_pid" 2>/dev/null || true
    wait "$graph_pid" 2>/dev/null || true
  fi
  rm -rf "$demo_home"
}
trap cleanup EXIT

export HOME="$demo_home"
export XDG_DATA_HOME="$demo_home/data"
export XDG_CONFIG_HOME="$demo_home/config"
export XDG_RUNTIME_DIR="$demo_home/run"
export MS_TODO_INSTANCE=demo
export MS_TODO_CLIENT_ID=00000000-0000-0000-0000-000000000000
export MS_TODO_GRAPH_URL="http://127.0.0.1:$port/v1.0"
unset MS_TODO_CONFIG_DIR
mkdir -p "$demo_home/bin" "$XDG_CONFIG_HOME/ms-todo"
ln -s "$target/ms-todo" "$demo_home/bin/ms-todo"
ln -s "$target/ms-todo" "$demo_home/bin/mst"
export PATH="$demo_home/bin:$PATH"

cat >"$XDG_CONFIG_HOME/ms-todo/config.toml" <<'EOF'
[tui]
theme = "catppuccin-mocha"
EOF

"$target/demo-fake-graph" "$repo/demo/seed.json" "$port" >"$demo_home/fake-graph.log" 2>&1 &
graph_pid=$!
for _ in $(seq 100); do
  grep -q "listening" "$demo_home/fake-graph.log" 2>/dev/null && break
  kill -0 "$graph_pid" 2>/dev/null || {
    cat "$demo_home/fake-graph.log" >&2
    exit 1
  }
  sleep 0.05
done

# A sign-in that's good for eight hours, so the daemon never refreshes. The
# token path comes from ms-todo itself, as the tests find it.
token_path="$(ms-todo --format json auth logout | sed -n 's/.*"token_path": *"\([^"]*\)".*/\1/p')"
expires_at=$(($(date +%s) + 8 * 3600))
(
  umask 077
  mkdir -p "$(dirname "$token_path")"
  cat >"$token_path" <<EOF
{"access_token":"demo-access-token","refresh_token":"demo-refresh-token","expires_at":$expires_at,"scopes":["Tasks.ReadWrite"],"client_id":"$MS_TODO_CLIENT_ID"}
EOF
)

ms-todo sync --wait >/dev/null

if [[ $# -gt 0 ]]; then
  "$@"
else
  echo "ms-todo demo: \`mst\` talks to a fake Graph as Alex Rivera. Exit the shell to clean up."
  PS1='demo $ ' BASH_SILENCE_DEPRECATION_WARNING=1 bash --norc -i
fi
