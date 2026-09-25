#!/usr/bin/env bash
# Re-record the README's GIFs against the fake Graph: each tape runs in its
# own demo/run.sh, so every recording starts from the same seed.
#
#   demo/record.sh          both tapes
#   demo/record.sh tui      just demo/tui.tape
#
# Needs vhs (with ttyd and ffmpeg), jq, and the JetBrainsMono Nerd Font.

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

for tool in vhs jq; do
  command -v "$tool" >/dev/null || {
    echo "demo/record.sh needs $tool on PATH" >&2
    exit 1
  }
done

tapes=("${@:-cli tui}")
# shellcheck disable=SC2206 # the default is two words on purpose
tapes=(${tapes[*]})
for tape in "${tapes[@]}"; do
  demo/run.sh -- vhs "demo/$tape.tape"
done
ls -la docs/assets/demo-*.gif
