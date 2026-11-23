#!/usr/bin/env bash
# Build the one Windows 11 installer from the already-built app.
#
#   ./scripts/build.sh
#   ./scripts/build-installer.sh
#
# The result is dist/GMO-TempTools-Setup.exe, built from dist/gmo-temptools.exe.
# It is a small WinForms installer that embeds the app, installs per-user under
# %LOCALAPPDATA%\Programs\GMO TempTools, creates shortcuts and optionally
# launches the app. It never bundles source or the build cache.

set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
APP="$ROOT/dist/gmo-temptools.exe"
SETUP="$ROOT/dist/GMO-TempTools-Setup.exe"
BUILD="$ROOT/.build"
CSC="${CSC:-/c/Windows/Microsoft.NET/Framework64/v4.0.30319/csc.exe}"
SOURCE="$ROOT/installer/Installer.cs"

if [ ! -f "$APP" ]; then
  echo "missing $APP — run ./scripts/build.sh first" >&2
  exit 2
fi
if [ ! -f "$SOURCE" ]; then
  echo "missing $SOURCE" >&2
  exit 2
fi
if [ ! -x "$CSC" ]; then
  echo "the .NET Framework C# compiler was not found at $CSC" >&2
  echo "Windows 11 includes it; set CSC to its full path if it is elsewhere" >&2
  exit 2
fi

CSC_WIN=$(cygpath -w "$CSC")
APP_WIN=$(cygpath -w "$APP")
SETUP_WIN=$(cygpath -w "$SETUP")
SOURCE_WIN=$(cygpath -w "$SOURCE")
RUN="$BUILD/compile-installer.cmd"
RUN_WIN=$(cygpath -w "$RUN")

mkdir -p "$ROOT/dist"
rm -rf "$BUILD"
mkdir -p "$BUILD"
rm -f "$SETUP"

cat >"$RUN" <<EOF
@echo off
"$CSC_WIN" /nologo /target:winexe /platform:anycpu /optimize+ /out:"$SETUP_WIN" /resource:"$APP_WIN",GMOApp.exe /reference:System.dll /reference:System.Drawing.dll /reference:System.Windows.Forms.dll "$SOURCE_WIN"
EOF
# cmd.exe is more reliable with CRLF-terminated batch files.
sed -i 's/$/\r/' "$RUN"

echo "compiling $SETUP ..."
MSYS_NO_PATHCONV=1 cmd.exe /c call "$RUN_WIN"
if [ ! -f "$SETUP" ]; then
  echo "the C# compiler did not produce $SETUP" >&2
  exit 1
fi
echo "installer built: $SETUP ($(wc -c <"$SETUP") bytes)"
rm -rf "$BUILD"
