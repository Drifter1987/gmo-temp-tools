#!/usr/bin/env bash
# Build the one GMO TempTools desktop app and refresh dist/gmo-temptools.exe.
#
#   ./scripts/build.sh
#
# The web assets are compiled first because Rust embeds them. A running app
# holds its .exe open on Windows; stop it before rebuilding.
#
# Set CARGO_TARGET_DIR to keep the multi-gigabyte build cache out of the
# project folder. The project itself should stay small; target/ is disposable.

set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP=gmo-temptools
DIST="$ROOT/dist"

cargo_bin="${USERPROFILE:-$HOME}/.cargo/bin"
if [ -d "$cargo_bin" ]; then
  PATH="$cargo_bin:$PATH"
fi

EXE=""
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    mingw="${USERPROFILE:-$HOME}/tools/mingw64/bin"
    if [ -d "$mingw" ]; then
      PATH="$mingw:$PATH"
    fi
    export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable-x86_64-pc-windows-gnu}"
    EXE=".exe"
    ;;
esac
export PATH

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is not on PATH; install the Rust toolchain or add it to this shell" >&2
  exit 2
fi
if ! command -v npm >/dev/null 2>&1; then
  echo "npm is not on PATH; it is required to compile the TypeScript browser assets" >&2
  exit 2
fi

printf 'web: typechecking and compiling TypeScript ... '
if ( cd "$ROOT" && npm run build:web --silent ); then
  echo "ok"
else
  echo "FAILED"
  exit 1
fi

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
echo "project: $ROOT"
echo "target cache: $TARGET_DIR"
echo "toolchain: ${RUSTUP_TOOLCHAIN:-default}"
echo

printf '%s: building ... ' "$APP"
if ( cd "$ROOT" && cargo build --release --target-dir "$TARGET_DIR" --quiet ); then
  echo "ok"
else
  echo "FAILED"
  exit 1
fi

src="$TARGET_DIR/release/$APP$EXE"
dst="$DIST/$APP$EXE"
mkdir -p "$DIST"
if [ ! -f "$src" ]; then
  echo "$APP: no build output at $src" >&2
  exit 1
fi
if cp -f "$src" "$dst" 2>/dev/null && cmp -s "$src" "$dst"; then
  echo "$APP: $APP$EXE refreshed"
else
  echo "$APP: could not replace $APP$EXE — it is probably running. Stop it and rerun." >&2
  exit 1
fi

echo
echo "app rebuilt: $dst"
echo "build the installer with: ./scripts/build-installer.sh"
