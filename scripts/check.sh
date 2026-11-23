#!/usr/bin/env bash
# One command for dead code and signature regressions.
#
#   TWILIO_AUTH_TOKEN=... TELNYX_PUBLIC_KEY=... WEBHOOK_SECRET=... ./scripts/check.sh
#
# Typechecks the TypeScript browser assets and Playwright specs, then compiles
# and runs the unit tests for the shared libraries and the unified GMO
# TempTools app with warnings treated as errors, so an unused function or
# variable fails the run.
#
# The app token is read from TEMPTOOLS_AUTH_TOKEN or .run/auth-token for the
# integration tests. The app does not need to be running for this script.

set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
AUTH_TOKEN="${TEMPTOOLS_AUTH_TOKEN:-}"
if [ -z "$AUTH_TOKEN" ] && [ -e "$ROOT/.run/auth-token" ]; then
  if ! AUTH_TOKEN=$(tr -d '\r\n' <"$ROOT/.run/auth-token"); then
    echo "could not read $ROOT/.run/auth-token" >&2
    exit 2
  fi
fi
export TEMPTOOLS_AUTH_TOKEN="$AUTH_TOKEN"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

cargo_bin="${USERPROFILE:-$HOME}/.cargo/bin"
if [ -d "$cargo_bin" ]; then
  PATH="$cargo_bin:$PATH"
fi
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    mingw="${USERPROFILE:-$HOME}/tools/mingw64/bin"
    if [ -d "$mingw" ]; then
      PATH="$mingw:$PATH"
    fi
    export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable-x86_64-pc-windows-gnu}"
    ;;
esac
export PATH
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-D warnings"

echo "== web TypeScript =="
if ! command -v npm >/dev/null 2>&1; then
  echo "npm is not on PATH; it is required to typecheck the TypeScript browser assets" >&2
  exit 2
fi
printf '%-14s ' "app.ts"
if ( cd "$ROOT" && npm run check:web --silent >"$WORK/web.log" 2>&1 ); then
  echo "typecheck ok"
else
  echo "FAILED"
  echo
  tail -25 "$WORK/web.log"
  exit 1
fi

printf '%-14s ' "e2e.ts"
if ( cd "$ROOT" && npm run check:e2e --silent >"$WORK/e2e.log" 2>&1 ); then
  echo "typecheck ok"
else
  echo "FAILED"
  echo
  tail -25 "$WORK/e2e.log"
  exit 1
fi
echo

echo "== unit tests (warnings are errors) =="
FAILED=0
for crate in webhook-sign local-auth desktop-notifications temp-ui temp-email temp-sms temp-otp; do
  printf '%-14s ' "$crate"
  if ( cd "$ROOT" && cargo test -p "$crate" --quiet >"$WORK/$crate.log" 2>&1 ); then
    passed=$(grep -oE '[0-9]+ passed' "$WORK/$crate.log" | awk '{ sum += $1 } END { print sum + 0 }')
    echo "${passed:-0} passed"
  else
    FAILED=1
    echo "FAILED"
    echo
    tail -25 "$WORK/$crate.log"
    echo
  fi
done

printf '%-14s ' "gmo-temptools"
if ( cd "$ROOT" && cargo test -p gmo-temptools --quiet >"$WORK/gmo-temptools.log" 2>&1 ); then
  passed=$(grep -oE '[0-9]+ passed' "$WORK/gmo-temptools.log" | awk '{ sum += $1 } END { print sum + 0 }')
  echo "${passed:-0} passed"
else
  FAILED=1
  echo "FAILED"
  echo
  tail -25 "$WORK/gmo-temptools.log"
  echo
fi

if [ "$FAILED" -ne 0 ]; then
  echo "tests: FAILED — the log above ends with the compile error (warnings are errors)"
  exit 1
fi
echo "tests: OK"
