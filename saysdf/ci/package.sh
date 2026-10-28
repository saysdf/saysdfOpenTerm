#!/usr/bin/env bash
# saysdfTerminal: package release binaries into dist/ (fork's own script; upstream ci/deploy.sh is untouched).
#   Windows -> saysdfTerm-<ver>-windows-x86_64.zip (incl. conpty.dll + OpenConsole.exe)
#   Linux   -> saysdfTerm-<ver>-linux-x86_64.tar.gz
#   macOS   -> saysdfTerm-<ver>-macos-<arch>.zip (contains saysdfTerm.app)
# Version: $VERSION, else tag from GITHUB_REF_NAME (v1.2.3 -> 1.2.3), else 0.0.0-<short-sha>.
# Env: TARGET_DIR (default target/release), DIST_DIR (default dist), PKG_OS (override detection).
set -euo pipefail
cd "$(dirname "$0")/../.."

APP=saysdfTerm
# Single source of binary names (see rebrand-saysdfterm).
BINS=(saysdfTerm saysdfTerm-cli saysdfTerm-mux-server saysdf)
# Optional binaries: packaged only if built.
OPT_BINS=()

TARGET_DIR="${TARGET_DIR:-target/release}"
DIST_DIR="${DIST_DIR:-dist}"

if [ -n "${VERSION:-}" ]; then ver="$VERSION"
elif [ "${GITHUB_REF_TYPE:-}" = tag ] || [[ "${GITHUB_REF_NAME:-}" == v* && "${GITHUB_REF:-}" == refs/tags/* ]]; then ver="${GITHUB_REF_NAME#v}"
else ver="0.0.0-$(git rev-parse --short=8 HEAD)"; fi
# guard against path characters in a tag name
[[ "$ver" =~ ^[0-9A-Za-z._+-]+$ ]] || { echo "bad version: $ver" >&2; exit 1; }

os="${PKG_OS:-}"
if [ -z "$os" ]; then
  case "${OSTYPE:-}" in
    msys*|cygwin*|win*) os=windows ;;
    darwin*) os=macos ;;
    linux*) os=linux ;;
    *) echo "unknown OSTYPE=${OSTYPE:-}" >&2; exit 1 ;;
  esac
fi
arch="$(uname -m)"; case "$arch" in x86_64|amd64) arch=x86_64 ;; arm64|aarch64) arch=aarch64 ;; esac
exe=""; [ "$os" = windows ] && exe=".exe"

name="$APP-$ver-$os-$arch"
mkdir -p "$DIST_DIR"
stage="$(mktemp -d)"; trap 'rm -rf "$stage"' EXIT

collect() { # $1 = destination dir
  local b
  for b in "${BINS[@]}"; do
    [ -f "$TARGET_DIR/$b$exe" ] || { echo "missing $TARGET_DIR/$b$exe" >&2; exit 1; }
    cp "$TARGET_DIR/$b$exe" "$1/"
  done
  for b in ${OPT_BINS[@]+"${OPT_BINS[@]}"}; do
    if [ -f "$TARGET_DIR/$b$exe" ]; then cp "$TARGET_DIR/$b$exe" "$1/"; else echo "skip optional $b$exe (not built)"; fi
  done
}

shell_integration() { # $1 = destination dir; scripts for saysdf.shell (change shell-integration, D2)
  mkdir -p "$1/shell-integration"
  cp -R saysdf/shell-integration/. "$1/shell-integration/"
  cp assets/shell-integration/wezterm.sh "$1/shell-integration/"
}

case "$os" in
  windows)
    mkdir -p "$stage/$name"; collect "$stage/$name"; shell_integration "$stage/$name"
    # Microsoft-signed ConPTY (as upstream ci/deploy.sh): without these next to the exe the terminal
    # falls back to the system conpty with its old bugs.
    for f in conpty.dll OpenConsole.exe; do
      [ -f "assets/windows/conhost/$f" ] || { echo "missing assets/windows/conhost/$f" >&2; exit 1; }
      cp "assets/windows/conhost/$f" "$stage/$name/"
    done
    cp LICENSE.md "$stage/$name/" 2>/dev/null || true
    out="$(cd "$DIST_DIR" && pwd)/$name.zip"
    if command -v 7z >/dev/null; then (cd "$stage" && 7z a -tzip -bd "$out" "$name" >/dev/null)
    else (cd "$stage" && powershell.exe -NoProfile -Command "Compress-Archive -Path '$name' -DestinationPath '$(cygpath -w "$out")' -Force"); fi
    ;;
  linux)
    mkdir -p "$stage/$name"; collect "$stage/$name"; shell_integration "$stage/$name"
    cp LICENSE.md "$stage/$name/" 2>/dev/null || true
    tar -C "$stage" -czf "$DIST_DIR/$name.tar.gz" "$name"
    ;;
  macos)
    app="$stage/$name/$APP.app"
    mkdir -p "$stage/$name"
    cp -R "assets/macos/$APP.app" "$app"
    mkdir -p "$app/Contents/MacOS"
    collect "$app/Contents/MacOS"
    shell_integration "$app/Contents/Resources"
    out="$(cd "$DIST_DIR" && pwd)/$name.zip"
    (cd "$stage/$name" && zip -qry "$out" "$APP.app")
    ;;
  *) echo "unsupported os: $os" >&2; exit 1 ;;
esac
echo "packaged: $(ls "$DIST_DIR" | grep -F "$name")"
