#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"

# Stop any running instance so the UI can start fresh.
if pgrep -x code-ui >/dev/null 2>&1; then
  pkill -TERM -x code-ui || true
  # Give it a moment to exit cleanly, then force if needed.
  sleep 0.2
  pkill -KILL -x code-ui >/dev/null 2>&1 || true
fi

# Rebuild if needed and relaunch the interface.
exec cargo run --quiet
