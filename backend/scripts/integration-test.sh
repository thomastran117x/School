#!/usr/bin/env bash
# Runs the backend against real MySQL, Redis, and OpenSearch containers.

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BACKEND_DIR="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
cd "${BACKEND_DIR}"

# A unique project name prevents this test from touching another Compose stack.
export COMPOSE_PROJECT_NAME="${COMPOSE_PROJECT_NAME:-rust-backend-ci-${GITHUB_RUN_ID:-$$}}"
export MYSQL_PORT="${MYSQL_PORT:-13306}"
export REDIS_PORT="${REDIS_PORT:-16379}"
export OPENSEARCH_PORT="${OPENSEARCH_PORT:-29200}"
export APP_PORT="${APP_PORT:-13000}"

export DATABASE_URL="mysql://app:app@127.0.0.1:${MYSQL_PORT}/app"
export DATABASE_ACQUIRE_TIMEOUT_MS="2000"
export DATABASE_QUERY_TIMEOUT_MS="5000"
export DATABASE_TLS_MODE="disabled"
export REDIS_TLS_MODE="disabled"
export REDIS_URL="redis://127.0.0.1:${REDIS_PORT}"
export OPENSEARCH_URL="http://127.0.0.1:${OPENSEARCH_PORT}"
export APP_HOST="127.0.0.1"
export RUST_LOG="backend_api=info"

TEST_TMP_DIR="$(mktemp -d)"
API_LOG="${TEST_TMP_DIR}/backend-api.log"
RESPONSE_BODY="${TEST_TMP_DIR}/response.json"
API_PID=""

cleanup() {
    local result=$?
    trap - EXIT INT TERM

    if (( result != 0 )); then
        echo "Integration test failed; collecting diagnostics."
        docker compose ps --all || true
        docker compose logs --no-color || true
        if [[ -f "${API_LOG}" ]]; then
            echo "Backend API log:"
            sed -n '1,300p' "${API_LOG}"
        fi
    fi

    if [[ -n "${API_PID}" ]] && kill -0 "${API_PID}" 2>/dev/null; then
        kill "${API_PID}" 2>/dev/null || true
        wait "${API_PID}" 2>/dev/null || true
    fi

    docker compose down --volumes --remove-orphans || true
    rm -rf -- "${TEST_TMP_DIR}"
    exit "${result}"
}
trap cleanup EXIT INT TERM

request_status() {
    local url=$1
    curl \
        --silent \
        --show-error \
        --output "${RESPONSE_BODY}" \
        --write-out '%{http_code}' \
        "${url}" || true
}

wait_for_status() {
    local url=$1
    local expected=$2
    local description=$3
    local attempts=${4:-60}

    for ((attempt = 1; attempt <= attempts; attempt++)); do
        if [[ "$(request_status "${url}")" == "${expected}" ]]; then
            return 0
        fi

        if [[ -n "${API_PID}" ]] && ! kill -0 "${API_PID}" 2>/dev/null; then
            echo "Backend API exited while waiting for ${description}." >&2
            return 1
        fi

        sleep 1
    done

    echo "Timed out waiting for ${description}; last response:" >&2
    cat "${RESPONSE_BODY}" >&2 || true
    return 1
}

echo "Starting integration-test datastores."
docker compose up \
    --detach \
    --wait \
    --wait-timeout 240 \
    mysql redis opensearch

echo "Checking the complete SeaORM migration lifecycle."
bash scripts/test-migrations.sh

echo "Checking MySQL connectivity, query deadlines, and pool exhaustion."
cargo test --locked -p backend-api --bin backend-api \
    infrastructure::database::tests::mysql_pool_enforces_tls_and_timeouts \
    -- --ignored --exact

echo "Checking Redis round trips, TTLs, and command deadlines."
cargo test --locked -p backend-api --bin backend-api \
    infrastructure::redis::tests::redis_round_trips_values_with_ttl \
    -- --ignored --exact

echo "Checking the general cache (NX, counters, JSON, atomic groups, scripts)."
cargo test --locked -p backend-api --bin backend-api \
    infrastructure::cache::tests::integration:: \
    -- --ignored

echo "Checking refresh-session rotation, replay, revocation, concurrency, and expiry."
cargo test --locked -p backend-api --bin backend-api \
    features::auth::refresh_store::service::tests::integration:: \
    -- --ignored

echo "Checking course cache-aside reads, invalidation, and stale-fill protection."
cargo test --locked -p backend-api --bin backend-api \
    features::courses::cache::tests::integration:: \
    -- --ignored

echo "Starting the backend API."
cargo build --locked -p backend-api
./target/debug/backend-api >"${API_LOG}" 2>&1 &
API_PID=$!

wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/live" \
    "200" \
    "the liveness endpoint"
grep -q '"status":"ok"' "${RESPONSE_BODY}"

wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/ready" \
    "200" \
    "all datastores to become ready" \
    120
grep -q '"mysql":"up"' "${RESPONSE_BODY}"
grep -q '"redis":"up"' "${RESPONSE_BODY}"
grep -q '"opensearch":"up"' "${RESPONSE_BODY}"

echo "Verifying degraded readiness when Redis is unavailable."
docker compose stop redis
wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/ready" \
    "503" \
    "Redis to be reported down"
grep -q '"mysql":"up"' "${RESPONSE_BODY}"
grep -q '"redis":"down"' "${RESPONSE_BODY}"
grep -q '"opensearch":"up"' "${RESPONSE_BODY}"

echo "Verifying readiness recovery after Redis restarts."
docker compose start redis
wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/ready" \
    "200" \
    "Redis readiness to recover" \
    120
grep -q '"redis":"up"' "${RESPONSE_BODY}"

echo "Verifying degraded readiness when MySQL is unavailable."
docker compose stop mysql
wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/live" \
    "200" \
    "liveness to remain available while MySQL is stopped"
wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/ready" \
    "503" \
    "MySQL to be reported down"
grep -q '"mysql":"down"' "${RESPONSE_BODY}"
grep -q '"redis":"up"' "${RESPONSE_BODY}"
grep -q '"opensearch":"up"' "${RESPONSE_BODY}"

echo "Verifying readiness recovery after MySQL restarts."
docker compose start mysql
wait_for_status \
    "http://127.0.0.1:${APP_PORT}/health/ready" \
    "200" \
    "MySQL readiness to recover" \
    120
grep -q '"mysql":"up"' "${RESPONSE_BODY}"

echo "Integration tests passed."

