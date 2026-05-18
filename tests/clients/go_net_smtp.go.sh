#!/usr/bin/env bash
# Wrapper so run-all.sh can `command -v go` and then invoke a script.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
go run "$here/go_net_smtp.go"
