#!/usr/bin/env bash
# Start, stop or inspect GMO TempTools, the single desktop app that combines
# temp-email, temp-sms and temp-otp.
#
#   ./scripts/run-all.sh            same as start
#   ./scripts/run-all.sh start      launch the app and wait until its APIs answer
#
#   ./scripts/run-all.sh stop       stop the app this script started
#   ./scripts/run-all.sh status     show whether the app and its APIs are running
#   ./scripts/run-all.sh token      print the local web/API token (keep it private)
#
# The app opens one native window with Mail, SMS (Beta), OTP and Settings, and
# serves the matching web app at http://127.0.0.1:8780/. Its APIs stay on the
# same ports as before: mail 8781 and texts 8782. Logs and the pid file live in
# .run/.
#
# The app is gated by a random per-install token stored at .run/auth-token.
# Open http://127.0.0.1:8780/ and unlock with that token; `./scripts/run-all.sh token`
# prints it for the local user. Carrier webhook credentials have no defaults:
# export TWILIO_AUTH_TOKEN, TELNYX_PUBLIC_KEY, WEBHOOK_SECRET or
# TWILIO_WEBHOOK_URL before starting to accept signed deliveries.

set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUN_DIR="$ROOT/.run"
LOG_DIR="$RUN_DIR/logs"
APP=gmo-temptools
EXE="$ROOT/dist/$APP.exe"
PID_FILE="$RUN_DIR/$APP.pid"
LOG="$LOG_DIR/$APP.log"
PORTS=(8780 8781 8782)

TWILIO_AUTH_TOKEN="${TWILIO_AUTH_TOKEN:-}"
TELNYX_PUBLIC_KEY="${TELNYX_PUBLIC_KEY:-}"
WEBHOOK_SECRET="${WEBHOOK_SECRET:-}"
TOKEN_FILE="$RUN_DIR/auth-token"
AUTH_TOKEN="${TEMPTOOLS_AUTH_TOKEN:-}"

load_token() {
  mkdir -p "$RUN_DIR"
  if [ -z "$AUTH_TOKEN" ] && [ -e "$TOKEN_FILE" ]; then
    if ! AUTH_TOKEN=$(tr -d '\r\n' <"$TOKEN_FILE"); then
      echo "could not read $TOKEN_FILE" >&2
      exit 1
    fi
  fi
  if [ -z "$AUTH_TOKEN" ]; then
    AUTH_TOKEN=$(od -An -N32 -tx1 /dev/urandom | tr -d ' \r\n')
    if [ -z "$AUTH_TOKEN" ]; then
      echo "could not generate .run/auth-token" >&2
      exit 1
    fi
    if ! (umask 077; printf '%s\n' "$AUTH_TOKEN" >"$TOKEN_FILE"); then
      echo "could not write $TOKEN_FILE" >&2
      exit 1
    fi
    chmod 600 "$TOKEN_FILE" 2>/dev/null || true
  elif [ -z "${TEMPTOOLS_AUTH_TOKEN:-}" ]; then
    # A file token is the persistent install token. Never rewrite it here;
    # tightening an old file is harmless and keeps the stored secret private.
    chmod 600 "$TOKEN_FILE" 2>/dev/null || true
  fi
  export TEMPTOOLS_AUTH_TOKEN="$AUTH_TOKEN"
}

load_token

pid_of() {
  [ -f "$PID_FILE" ] && cat "$PID_FILE" 2>/dev/null
}

is_running() {
  local pid
  pid=$(pid_of)
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

answers() {
  local code
  code=$(curl -s --max-time 2 -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $AUTH_TOKEN" \
    "http://127.0.0.1:$1/api/status" 2>/dev/null || true)
  [ "$code" = "200" ]
}

stop_app() {
  local pid
  pid=$(pid_of)
  if [ -z "$pid" ]; then
    rm -f "$PID_FILE"
    printf '%-14s not tracked (nothing to stop)\n' "$APP"
    return 0
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    rm -f "$PID_FILE"
    printf '%-14s already stopped\n' "$APP"
    return 0
  fi
  kill "$pid" 2>/dev/null
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.2
  done
  kill -9 "$pid" 2>/dev/null
  command -v taskkill >/dev/null 2>&1 && taskkill //F //PID "$pid" >/dev/null 2>&1
  wait "$pid" 2>/dev/null
  rm -f "$PID_FILE"
  printf '%-14s stopped (pid %s)\n' "$APP" "$pid"
}

start_app() {
  local port pid i
  if is_running; then
    printf '%-14s already running (pid %s)\n' "$APP" "$(pid_of)"
    return 0
  fi
  if [ ! -x "$EXE" ]; then
    printf '%-14s missing %s — run ./scripts/build.sh first\n' "$APP" "$EXE" >&2
    return 1
  fi
  for port in "${PORTS[@]}"; do
    if answers "$port"; then
      printf '%-14s port %s is already answering — stop that instance first\n' "$APP" "$port" >&2
      return 1
    fi
  done

  local envs=(env -u PORT -u TWILIO_WEBHOOK_URL
    "TEMPTOOLS_AUTH_TOKEN=$AUTH_TOKEN"
    "TEMPTOOLS_WEB_PORT=8780")
  [ -n "$TWILIO_AUTH_TOKEN" ] && envs+=("TWILIO_AUTH_TOKEN=$TWILIO_AUTH_TOKEN")
  [ -n "$TELNYX_PUBLIC_KEY" ] && envs+=("TELNYX_PUBLIC_KEY=$TELNYX_PUBLIC_KEY")
  [ -n "$WEBHOOK_SECRET" ] && envs+=("WEBHOOK_SECRET=$WEBHOOK_SECRET")
  [ -n "${TWILIO_WEBHOOK_URL:-}" ] && envs+=("TWILIO_WEBHOOK_URL=$TWILIO_WEBHOOK_URL")

  (
    cd "$ROOT" || exit 1
    "${envs[@]}" "$EXE" >"$LOG" 2>&1 &
    echo $! >"$PID_FILE"
  )
  pid=$(pid_of)

  for i in $(seq 1 80); do
    local ready=1
    for port in "${PORTS[@]}"; do
      answers "$port" || ready=0
    done
    if [ "$ready" -eq 1 ]; then
      printf '%-14s ready  pid %s\n' "$APP" "$pid"
      for port in "${PORTS[@]}"; do
        label=api
        [ "$port" = "8780" ] && label=web
        printf '  %-8s http://127.0.0.1:%s\n' "$label" "$port"
      done
      return 0
    fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.25
  done

  printf '%-14s did not answer on all API ports; log:\n' "$APP" >&2
  tail -12 "$LOG" >&2
  stop_app >/dev/null
  return 1
}

command="${1:-start}"
case "$command" in
  start)
    mkdir -p "$LOG_DIR"
    start_app
    echo
    echo "native window + web app at http://127.0.0.1:8780/; unlock with: ./scripts/run-all.sh token"
    echo "stop with: ./scripts/run-all.sh stop"
    ;;
  stop)
    stop_app
    ;;
  token)
    printf '%s\n' "$AUTH_TOKEN"
    ;;
  status)
    if is_running; then
      printf '%-14s running  pid %-6s one window\n' "$APP" "$(pid_of)"
      for port in "${PORTS[@]}"; do
        label=api
        [ "$port" = "8780" ] && label=web
        if answers "$port"; then
          printf '  %-8s http://127.0.0.1:%s\n' "$label" "$port"
        else
          printf '  %-8s not answering\n' "$label"
        fi
      done
    else
      printf '%-14s stopped\n' "$APP"
    fi
    ;;
  -h | --help | help)
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    ;;
  *)
    echo "unknown command: $command (use start, stop, status or token)" >&2
    exit 2
    ;;
esac
