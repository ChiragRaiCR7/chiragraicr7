#!/usr/bin/env bash
# Runs the e2e suite: Rust server (site + API) on :8080, plain static host on :8090.
# Expects server/target/debug/portfolio-server and web/dist to be built, and a
# .venv with tests/e2e/requirements.txt installed.
set -euo pipefail
cd "$(dirname "$0")/../.."
work="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null || true; rm -rf "$work"' EXIT

DATABASE_PATH="$work/e2e.db" STATIC_DIR=web/dist PORT=8080 server/target/debug/portfolio-server &
python3 -m http.server 8090 --bind 127.0.0.1 --directory web/dist >/dev/null 2>&1 &

for port in 8080 8090; do
  for _ in $(seq 1 50); do
    curl -fsS -o /dev/null "http://127.0.0.1:$port/" && break
    sleep 0.2
  done
done

.venv/bin/python tests/e2e/test_portfolio.py \
  http://127.0.0.1:8080/ http://127.0.0.1:8090/ "$work/e2e.db" "${SHOTS_DIR:-$work/shots}"
