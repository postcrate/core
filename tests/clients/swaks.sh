#!/usr/bin/env bash
# Send one message through swaks. Requires POSTCRATE_SMTP_HOST/PORT env.
set -euo pipefail
swaks --to "rcpt-swaks@example.com" \
      --from "swaks@example.com" \
      --server "$POSTCRATE_SMTP_HOST:$POSTCRATE_SMTP_PORT" \
      --header "Subject: swaks test" \
      --body "Hello from swaks." \
      >/dev/null
