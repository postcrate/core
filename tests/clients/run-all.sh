#!/usr/bin/env bash
# Interop smoke: boot postcrate-ci, run each client that's available,
# assert all captures land. Fails (non-zero exit) on any miss.
#
# Skips clients whose runtime isn't installed (python3 / node / go).
# `swaks` is the only one without a graceful skip path — we report it
# as a skip too if not on PATH.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# postcrate-core/tests/clients/ — workspace root is two levels up.
repo="$(cd "$here/../.." && pwd)"

# Build release binary so cold-start is realistic.
cd "$repo"
cargo build --release -p postcrate-ci >/dev/null

# Pick high random-ish ports so parallel CI runs don't fight.
smtp_port=$((24025 + RANDOM % 1000))
http_port=$((24080 + RANDOM % 1000))

tmp="$(mktemp -d -t postcrate-interop.XXXXXX)"
trap 'rm -rf "$tmp"; kill "$pid" 2>/dev/null || true' EXIT

# Spawn postcrate-ci in the background and capture its banner.
"$repo/target/release/postcrate-ci" \
  --smtp "$smtp_port" --http "$http_port" --bind 127.0.0.1 \
  --data-dir "$tmp/data" >"$tmp/banner" 2>"$tmp/log" &
pid=$!

# Wait for the banner (max 5s).
for _ in $(seq 1 50); do
  if grep -q POSTCRATE_API_URL "$tmp/banner" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
if ! grep -q POSTCRATE_API_URL "$tmp/banner"; then
  echo "postcrate-ci never reported ready"
  cat "$tmp/log"
  exit 1
fi

# shellcheck disable=SC1090
set -a; . "$tmp/banner"; set +a
echo "Booted: SMTP=$POSTCRATE_SMTP_HOST:$POSTCRATE_SMTP_PORT  API=$POSTCRATE_API_URL"

# Track how many we expect.
expected=0
ran=()

run_client() {
  local label=$1 cmd=$2
  if eval "command -v $cmd" >/dev/null 2>&1; then
    echo "  → $label"
    if "$here/$3"; then
      expected=$((expected + 1))
      ran+=("$label")
    else
      echo "  $label client failed"
      exit 1
    fi
  else
    echo "  ~ skipping $label ($cmd not found)"
  fi
}

run_client swaks                swaks       swaks.sh
run_client python_smtplib       python3     python_smtplib.py
run_client node_nodemailer      node        node_nodemailer.js
run_client go_net_smtp          go          go_net_smtp.go.sh

if [[ $expected -eq 0 ]]; then
  echo "no clients available; nothing to assert"
  exit 0
fi

# Poll the HTTP API for captures.
deadline=$(( $(date +%s) + 5 ))
while true; do
  count=$(curl -fsS "$POSTCRATE_API_URL/api/v1/messages?mailboxId=$(
    curl -fsS "$POSTCRATE_API_URL/api/v1/mailboxes?projectId=ci" \
      | python3 -c 'import json,sys;print(json.load(sys.stdin)[0]["id"])'
  )" | python3 -c 'import json,sys;print(len(json.load(sys.stdin)))')
  if [[ $count -ge $expected ]]; then
    echo "captured $count/$expected messages from: ${ran[*]}"
    exit 0
  fi
  if [[ $(date +%s) -ge $deadline ]]; then
    echo "only captured $count/$expected within 5s"
    exit 1
  fi
  sleep 0.1
done
