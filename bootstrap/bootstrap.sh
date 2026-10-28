#!/usr/bin/env bash
# saysdfTerm bootstrap for Linux and macOS: build from source and install for the current user.
#
#   bash bootstrap/bootstrap.sh [--dry-run] [--yes] [--dir DIR] [--ref TAG]
#   bash bootstrap/bootstrap.sh --uninstall
#
# Steps (each is printed before it runs; nothing is installed without asking, --yes answers "yes"):
#   1. prerequisites: git, a C toolchain, Rust (rustup); on Linux the system packages are installed
#      by the repository's ./get-deps (it uses sudo/doas and your package manager), on macOS the
#      Xcode command line tools are checked;
#   2. source: the checkout this script is in, or a clone of https://github.com/saysdf/saysdfOpenTerm.git
#      into --dir (default ~/src/saysdfOpenTerm) at --ref (default: the latest v* tag);
#   3. release build with a job limit that leaves >= 10% of RAM and CPU free, at low priority (nice);
#   4. install without root: Linux ~/.local/lib/saysdfTerm + links in ~/.local/bin + .desktop + icon;
#      macOS ~/Applications/saysdfTerm.app + links in ~/.local/bin.
# --dry-run prints the plan and changes nothing. --uninstall removes what step 4 installed
# (your config in ~/.config/saysdfTerm is kept).
set -euo pipefail

REPO_URL="https://github.com/saysdf/saysdfOpenTerm.git"
APP=saysdfTerm
APP_ID=com.saysdf.saysdfTerm
# Binaries and cargo packages: same lists as saysdf/tools/build-env.ps1 and saysdf/ci/package.sh.
BINS="saysdfTerm saysdfTerm-cli saysdfTerm-mux-server saysdf"
PACKAGES="wezterm-gui wezterm wezterm-mux-server saysdf-cli"
CONFIG_PATH="$HOME/.config/saysdfTerm/saysdfTerm.lua"

DRY_RUN=0
YES=0
UNINSTALL=0
DIR="$HOME/src/saysdfOpenTerm"
REF=""

usage() {
  sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
  case "$1" in
    -n|--dry-run) DRY_RUN=1 ;;
    -y|--yes) YES=1 ;;
    --uninstall) UNINSTALL=1 ;;
    --dir) shift; DIR="${1:?--dir needs a value}" ;;
    --dir=*) DIR="${1#--dir=}" ;;
    --ref) shift; REF="${1:?--ref needs a value}" ;;
    --ref=*) REF="${1#--ref=}" ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
  shift
done

step() { printf '\n==> %s\n' "$*"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '    WARNING: %s\n' "$*" >&2; }
die() { printf '\nERROR: %s\n' "$*" >&2; exit 1; }

# Print a command, then run it (not in a dry run).
run() {
  printf '    > %s\n' "$*"
  if [ "$DRY_RUN" -eq 0 ]; then "$@"; fi
}

# Same in another directory (subshell; the caller's directory does not change).
run_in() {
  local dir="$1"; shift
  printf '    > (cd %s) %s\n' "$dir" "$*"
  if [ "$DRY_RUN" -eq 0 ]; then (cd "$dir" && "$@"); fi
}

# Ask yes/no. --yes answers yes; a dry run prints the question and assumes yes.
ask() {
  if [ "$DRY_RUN" -eq 1 ]; then info "[dry run] would ask: $1"; return 0; fi
  if [ "$YES" -eq 1 ]; then info "$1 -> yes (--yes)"; return 0; fi
  local ans=""
  if [ -r /dev/tty ]; then
    printf '    %s [y/N] ' "$1" > /dev/tty
    read -r ans < /dev/tty || ans=""
  else
    warn "no terminal to ask: \"$1\" -> no (use --yes)"
  fi
  case "$ans" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

have() { command -v "$1" >/dev/null 2>&1; }

OS="$(uname -s)"
case "$OS" in
  Linux) OS=linux ;;
  Darwin) OS=macos ;;
  *) die "unsupported OS: $OS (use bootstrap.ps1 on Windows)" ;;
esac

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
if [ "$OS" = linux ]; then
  # Not ~/.local/share/saysdfTerm: that is the terminal's own data directory (logs, sockets, state).
  DEST="$HOME/.local/lib/$APP"
  DESKTOP_FILE="$DATA_HOME/applications/$APP_ID.desktop"
  ICON_FILE="$DATA_HOME/icons/hicolor/128x128/apps/$APP_ID.png"
  BIN_SRC_DIR="$DEST"
else
  DEST="$HOME/Applications/$APP.app"
  DESKTOP_FILE=""
  ICON_FILE=""
  BIN_SRC_DIR="$DEST/Contents/MacOS"
fi

echo "saysdfTerm bootstrap ($OS)"
[ "$DRY_RUN" -eq 1 ] && echo "Dry run: nothing will be installed, cloned, built or changed."

# ---------------------------------------------------------------- uninstall
remove_links() {
  local b link
  for b in $BINS; do
    link="$BIN_DIR/$b"
    if [ -L "$link" ]; then
      case "$(readlink "$link")" in
        "$BIN_SRC_DIR"/*) run rm -f "$link" ;;
        *) info "kept $link (points elsewhere)" ;;
      esac
    fi
  done
}

if [ "$UNINSTALL" -eq 1 ]; then
  step "Uninstall"
  if [ -e "$DEST" ]; then
    [ -x "$BIN_SRC_DIR/saysdfTerm" ] || die "$DEST does not look like a saysdfTerm install; not deleting it"
    run rm -rf "$DEST"
  else
    info "$DEST does not exist"
  fi
  remove_links
  if [ -n "$DESKTOP_FILE" ] && [ -e "$DESKTOP_FILE" ]; then run rm -f "$DESKTOP_FILE"; fi
  if [ -n "$ICON_FILE" ] && [ -e "$ICON_FILE" ]; then run rm -f "$ICON_FILE"; fi
  if [ "$OS" = linux ] && have update-desktop-database; then run update-desktop-database "$DATA_HOME/applications" || true; fi
  info "Your configuration (~/.config/saysdfTerm) and the source checkout were not touched."
  exit 0
fi

# ---------------------------------------------------------------- 1. prerequisites
step "1/4 Prerequisites"
MISSING=""

if [ "$OS" = macos ]; then
  if xcode-select -p >/dev/null 2>&1; then
    info "Xcode command line tools: $(xcode-select -p)"
  elif ask "Install the Xcode command line tools (compiler, git) with 'xcode-select --install'? It opens a system dialog."; then
    run xcode-select --install || true
    [ "$DRY_RUN" -eq 1 ] || die "Finish the Xcode command line tools installation in the dialog, then run this script again."
  else
    MISSING="$MISSING\n  - Xcode command line tools: xcode-select --install"
  fi
fi

if have git; then
  info "git: $(git --version)"
else
  if [ "$OS" = linux ]; then
    MISSING="$MISSING\n  - git: install it with your package manager, e.g. 'sudo apt install git' or 'sudo dnf install git'"
  else
    MISSING="$MISSING\n  - git: comes with the Xcode command line tools"
  fi
fi

if [ -x "$HOME/.cargo/bin/cargo" ] && ! have cargo; then export PATH="$HOME/.cargo/bin:$PATH"; fi
if have cargo; then
  info "Rust: $(cargo --version)"
else
  if ask "Rust is missing. Download the official rustup installer from https://sh.rustup.rs (rust-lang.org) and run it (installs into ~/.rustup and ~/.cargo, no root)?"; then
    have curl || die "curl is needed to download rustup; install curl or Rust (https://rustup.rs) yourself"
    tmp_rustup="$(mktemp "${TMPDIR:-/tmp}/rustup-init.XXXXXX")"
    run curl --proto '=https' --tlsv1.2 -sSf -o "$tmp_rustup" https://sh.rustup.rs
    run sh "$tmp_rustup" -y --profile minimal
    rm -f "$tmp_rustup"
    export PATH="$HOME/.cargo/bin:$PATH"
  else
    MISSING="$MISSING\n  - Rust: https://rustup.rs"
  fi
fi

if [ -n "$MISSING" ]; then
  printf '    Missing:%b\n' "$MISSING"
  [ "$DRY_RUN" -eq 1 ] || die "Install the missing tools and run the script again."
fi

# ---------------------------------------------------------------- 2. source
step "2/4 Source"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HERE="$(dirname "$SCRIPT_DIR")"
if [ -f "$HERE/Cargo.toml" ] && [ -f "$HERE/saysdf/ci/package.sh" ]; then
  SRC="$HERE"
  info "Using the checkout this script is in: $SRC"
  [ -n "$REF" ] && warn "--ref is used only for a fresh clone; check out '$REF' in $SRC yourself if you want it."
  if git -C "$SRC" submodule status 2>/dev/null | grep -q '^-'; then
    run git -C "$SRC" submodule update --init --recursive
  else
    info "Submodules: initialized"
  fi
else
  SRC="$DIR"
  if [ -d "$SRC/.git" ]; then
    info "Updating the existing clone in $SRC"
    run git -C "$SRC" fetch --tags origin
  else
    info "Cloning $REPO_URL into $SRC"
    run mkdir -p "$(dirname "$SRC")"
    run git clone --recurse-submodules "$REPO_URL" "$SRC"
  fi
  if [ -z "$REF" ]; then
    if [ -d "$SRC/.git" ]; then
      REF="$(git -C "$SRC" tag --list 'v*' --sort=-v:refname | head -n 1)"
      if [ -z "$REF" ]; then REF=origin/HEAD; warn "no release tags yet: building the default branch"; fi
    else
      REF="<latest v* tag>"
    fi
  fi
  info "Ref: $REF"
  run git -C "$SRC" checkout --quiet "$REF"
  run git -C "$SRC" submodule update --init --recursive
fi

if [ "$OS" = linux ]; then
  info "System packages for the build are installed by ./get-deps (apt, dnf, zypper, pacman, apk, ...)."
  if ask "Run ./get-deps now? It installs packages through sudo (or doas) and may ask for your password."; then
    if [ "$DRY_RUN" -eq 1 ] || [ -x "$SRC/get-deps" ]; then
      run_in "$SRC" ./get-deps
    else
      die "$SRC/get-deps not found"
    fi
  else
    info "Skipped; if the build fails on missing libraries, run ./get-deps in $SRC."
  fi
fi

# ---------------------------------------------------------------- 3. build
step "3/4 Build (release; the first build takes 20-40 minutes)"
cores="$( (nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2) | head -n 1)"
total_mb=""; free_mb=""
if [ -r /proc/meminfo ]; then
  total_mb=$(awk '/^MemTotal:/ {print int($2/1024)}' /proc/meminfo)
  free_mb=$(awk '/^MemAvailable:/ {print int($2/1024)}' /proc/meminfo)
elif have vm_stat; then
  total_mb=$(( $(sysctl -n hw.memsize) / 1048576 ))
  page=$(vm_stat | awk 'NR==1 {gsub(/[^0-9]/, ""); print}')
  pages=$(vm_stat | awk '/^Pages (free|inactive|speculative):/ {gsub(/\./, "", $NF); s += $NF} END {print s+0}')
  free_mb=$(( pages * page / 1048576 ))
fi
# Same formula as saysdf/tools/build-env.ps1: 90% of cores (at least one core free) and
# (free RAM - 10% of total) / 1.5 GB per rustc job.
cpu_jobs=$(( cores * 9 / 10 )); [ "$cpu_jobs" -gt $(( cores - 1 )) ] && cpu_jobs=$(( cores - 1 )); [ "$cpu_jobs" -lt 1 ] && cpu_jobs=1
if [ -n "$total_mb" ] && [ -n "$free_mb" ]; then
  mem_jobs=$(( (free_mb - total_mb / 10) / 1536 )); [ "$mem_jobs" -lt 1 ] && mem_jobs=1
  ram_info="free RAM ${free_mb} MB of ${total_mb} MB"
else
  mem_jobs=2; ram_info="free RAM unknown"
fi
jobs=$cpu_jobs; [ "$mem_jobs" -lt "$jobs" ] && jobs=$mem_jobs
if [ -n "${CARGO_BUILD_JOBS:-}" ]; then
  info "Parallel jobs: $CARGO_BUILD_JOBS (from CARGO_BUILD_JOBS)"
else
  export CARGO_BUILD_JOBS=$jobs
  info "Parallel jobs: $jobs ($ram_info, cores $cores); cargo runs with nice -n 10"
fi
TARGET_DIR="${CARGO_TARGET_DIR:-$SRC/target}"
export CARGO_TARGET_DIR="$TARGET_DIR"
info "Build directory: $TARGET_DIR"
set --
for p in $PACKAGES; do set -- "$@" -p "$p"; done
run_in "$SRC" nice -n 10 cargo build --release "$@"

# ---------------------------------------------------------------- 4. install
step "4/4 Install (current user, no root)"
info "Install to: $DEST"
info "Links:      $BIN_DIR/{$(echo "$BINS" | tr ' ' ',')}"
[ -n "$DESKTOP_FILE" ] && info "Menu entry: $DESKTOP_FILE, icon $ICON_FILE"

# Version label for the package name only.
ver="$(git -C "$SRC" describe --tags --match 'v*' 2>/dev/null || git -C "$SRC" rev-parse --short HEAD 2>/dev/null || echo 0.0.0)"
ver="$(printf '%s' "${ver#v}" | tr -c '0-9A-Za-z._+-' '-')"

if [ "$DRY_RUN" -eq 1 ]; then
  info "[dry run] would package with saysdf/ci/package.sh (TARGET_DIR=$TARGET_DIR/release), unpack and swap it into $DEST atomically"
else
  stage="$(mktemp -d "${TMPDIR:-/tmp}/saysdfTerm-install.XXXXXX")"
  trap 'rm -rf "$stage"' EXIT
  # saysdf/ci/package.sh assembles the same layout as the release archives (binaries, shell
  # integration, license; the .app bundle on macOS).
  run env TARGET_DIR="$TARGET_DIR/release" DIST_DIR="$stage/dist" VERSION="$ver" bash "$SRC/saysdf/ci/package.sh"
  mkdir -p "$stage/unpacked"
  if [ "$OS" = linux ]; then
    archive="$(ls "$stage/dist"/*.tar.gz)"
    run tar -xzf "$archive" -C "$stage/unpacked"
    new_dir="$(find "$stage/unpacked" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  else
    archive="$(ls "$stage/dist"/*.zip)"
    run ditto -x -k "$archive" "$stage/unpacked"
    new_dir="$stage/unpacked/$APP.app"
  fi
  [ -x "$new_dir/saysdfTerm" ] || [ -x "$new_dir/Contents/MacOS/saysdfTerm" ] || die "package layout not as expected in $new_dir"

  # Atomic swap: the new tree next to the old one, then two renames; the old one comes back on failure.
  run mkdir -p "$(dirname "$DEST")"
  rm -rf "$DEST.new" "$DEST.old"
  run mv "$new_dir" "$DEST.new"
  if [ -e "$DEST" ]; then run mv "$DEST" "$DEST.old"; fi
  if ! mv "$DEST.new" "$DEST"; then
    [ -e "$DEST.old" ] && mv "$DEST.old" "$DEST"
    die "could not move the new version into $DEST; the previous version was kept"
  fi
  rm -rf "$DEST.old"
fi

run mkdir -p "$BIN_DIR"
for b in $BINS; do
  link="$BIN_DIR/$b"
  if [ -e "$link" ] && [ ! -L "$link" ]; then
    warn "$link exists and is not a link; not replacing it"
    continue
  fi
  run ln -sfn "$BIN_SRC_DIR/$b" "$link"
done

if [ "$OS" = linux ]; then
  run mkdir -p "$(dirname "$DESKTOP_FILE")" "$(dirname "$ICON_FILE")"
  if [ "$DRY_RUN" -eq 0 ]; then
    # assets/saysdfTerm.desktop with absolute paths (~/.local/bin is often not on the desktop's PATH)
    sed -e "s|^TryExec=.*|TryExec=$DEST/saysdfTerm|" \
        -e "s|^Exec=saysdfTerm |Exec=\"$DEST/saysdfTerm\" |" \
        "$SRC/assets/saysdfTerm.desktop" > "$DESKTOP_FILE"
  else
    info "[dry run] would write $DESKTOP_FILE from assets/saysdfTerm.desktop"
  fi
  run cp "$SRC/assets/icon/terminal.png" "$ICON_FILE"
  if have update-desktop-database; then run update-desktop-database "$DATA_HOME/applications" || true; fi
fi

echo
if [ "$DRY_RUN" -eq 1 ]; then echo "Dry run finished: nothing was changed."; else echo "Done."; fi
echo "  Installed to:  $DEST"
echo "  Commands:      $BIN_DIR/{$(echo "$BINS" | tr ' ' ',')}"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) echo "                 $BIN_DIR is not on your PATH; add it in your shell profile" ;;
esac
if [ "$OS" = linux ]; then
  echo "  Start:         'saysdfTerm' from the application menu, or run: saysdfTerm"
else
  echo "  Start:         open ~/Applications/$APP.app (Launchpad / Spotlight), or run: saysdfTerm"
fi
echo "  Config file:   $CONFIG_PATH   (create it to customize; see README.md)"
echo "  Source:        $SRC"
echo "  Update:        run this script again; uninstall: $0 --uninstall"
