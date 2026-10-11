#!/usr/bin/env bash
# Regenerate the README/docs screenshots of the desktop app.
#   mise run gui:screenshots [-- extra harness args, e.g. --only query]
# With Docker available, a throwaway PostgreSQL is started so the query,
# plan (EXPLAIN ANALYZE) and inspector shots use a real PostgreSQL plan;
# without Docker everything runs against the seeded SQLite demo database.
set -euo pipefail

cd "$(dirname "$0")/../.."
compose=(docker compose -p dbcrust-gui-screenshots -f gui/scripts/screenshots-compose.yaml)
port="${DBCRUST_SHOTS_PG_PORT:-55432}"
args=(--out docs/public/screenshots --demo-dir /tmp/dbcrust-demo)

if docker info >/dev/null 2>&1; then
    trap '"${compose[@]}" down -v >/dev/null 2>&1 || true' EXIT
    "${compose[@]}" up -d --wait
    args+=(--postgres "postgres://shop:shop@127.0.0.1:${port}/shop")
else
    echo "Docker not available: rendering with SQLite only" >&2
fi

harness=(cargo run -p dbcrust-gui --features screenshots --bin dbcrust-gui-screenshots --)
"${harness[@]}" "${args[@]}" "$@"

# Connections page states shown in the GUI guide (no database needed)
if [ "$#" -eq 0 ]; then
    for state in home-new home-docker-failed; do
        "${harness[@]}" --out docs/public/screenshots --demo-dir /tmp/dbcrust-demo --only "$state"
    done
fi
