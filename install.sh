#!/bin/sh
# Adapted from spotuify install.sh @ d807e5e4f9d2f09878cdc22309af3589623f7785
# Rewritten as POSIX sh so `curl -fsSL .../install.sh | sh` works where sh is dash.
set -eu

repo="planetaryescape/ms-todo"
install_dir="${MS_TODO_INSTALL_DIR:-$HOME/.local/bin}"
version="${MS_TODO_VERSION:-latest}"

usage() {
  cat <<'EOF'
usage: install.sh [--version <version>] [--dir <install-dir>]

Installs the ms-todo release archive for this OS and architecture, and checks
it against the .sha256 file published with the GitHub release first.

Environment:
  MS_TODO_VERSION      Release version, e.g. v0.1.0. Defaults to latest.
  MS_TODO_INSTALL_DIR  Install directory. Defaults to ~/.local/bin.
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || { echo "--version requires a value" >&2; exit 64; }
      version="$2"
      shift 2
      ;;
    --dir)
      [ $# -ge 2 ] || { echo "--dir requires a value" >&2; exit 64; }
      install_dir="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required" >&2
    exit 69
  fi
}

need curl
need tar

case "$(uname -s):$(uname -m)" in
  Darwin:arm64) platform="macos-aarch64" ;;
  Darwin:x86_64) platform="macos-x86_64" ;;
  Linux:x86_64) platform="linux-x86_64" ;;
  *)
    echo "no prebuilt ms-todo archive for $(uname -s) $(uname -m)" >&2
    echo "try: cargo install --git https://github.com/$repo --locked ms-todo" >&2
    exit 69
    ;;
esac

if [ "$version" = "latest" ]; then
  latest_url="$(curl -fsSIL -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")"
  version="${latest_url##*/}"
  case "$version" in
    v[0-9]*) ;;
    *)
      echo "could not find the latest ms-todo release (got $latest_url)" >&2
      exit 69
      ;;
  esac
fi

release_version="${version#v}"
tag="v${release_version}"
archive="ms-todo-${tag}-${platform}.tar.gz"
base_url="https://github.com/$repo/releases/download/$tag"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT INT TERM

curl -fL --proto '=https' --tlsv1.2 -o "$tmpdir/$archive" "$base_url/$archive"
curl -fL --proto '=https' --tlsv1.2 -o "$tmpdir/$archive.sha256" "$base_url/$archive.sha256"

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$tmpdir" && sha256sum -c "$archive.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$tmpdir" && shasum -a 256 -c "$archive.sha256")
else
  echo "sha256sum or shasum is required to verify $archive" >&2
  exit 69
fi

tar -xzf "$tmpdir/$archive" -C "$tmpdir"
if [ ! -x "$tmpdir/ms-todo" ]; then
  echo "archive did not contain an executable ms-todo binary" >&2
  exit 65
fi

mkdir -p "$install_dir"
install -m 0755 "$tmpdir/ms-todo" "$install_dir/ms-todo"
echo "installed ms-todo $tag to $install_dir/ms-todo"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) echo "note: $install_dir is not on your PATH" >&2 ;;
esac
