#!/usr/bin/env bash
#
# test-infra.test.sh — TDD test harness for US-701: Docker Compose test dependency stack.
#
# Validates:
#   1. docker-compose.yml structure (postgres, redis, mock-registry)
#   2. seed.sql schema (test_users with 5 rows, test_analytics table)
#   3. wait-for-deps.sh behavior (polls services, timeout handling)
#   4. Full lifecycle: bring up → wait → SELECT 1 + PING → tear down
#   5. tmpfs configured for data directories
#
# Usage:
#   test-infra/test-infra.test.sh              # Run all tests
#   test-infra/test-infra.test.sh --unit       # Unit tests only (no Docker)
#   test-infra/test-infra.test.sh --integration # Integration tests (requires Docker)
#
# Exit codes:
#   0  All tests passed
#   1  One or more tests failed
#   2  Prerequisites missing

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_INFRA="${PROJECT_ROOT}/test-infra"

# Counters
TOTAL=0
PASSED=0
FAILED=0
SKIPPED=0

# Colors (if terminal supports it)
if [ -t 1 ]; then
  RED='\033[0;31m'
  GREEN='\033[0;32m'
  YELLOW='\033[0;33m'
  BLUE='\033[0;34m'
  NC='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' NC=''
fi

# ─── Test helpers ──────────────────────────────────────────────────────────────

pass() {
  TOTAL=$((TOTAL + 1))
  PASSED=$((PASSED + 1))
  printf "  ${GREEN}✓${NC} %s\n" "$1"
}

fail() {
  TOTAL=$((TOTAL + 1))
  FAILED=$((FAILED + 1))
  printf "  ${RED}✗${NC} %s\n" "$1"
  if [ -n "${2:-}" ]; then
    printf "    ${RED}→ %s${NC}\n" "$2"
  fi
}

skip() {
  TOTAL=$((TOTAL + 1))
  SKIPPED=$((SKIPPED + 1))
  printf "  ${YELLOW}○${NC} %s (skipped: %s)\n" "$1" "$2"
}

section() {
  printf "\n${BLUE}── %s ──${NC}\n" "$1"
}

# ─── Unit Tests: File existence and structure ──────────────────────────────────

test_unit() {
  section "File existence"

  # docker-compose.yml exists
  if [ -f "${TEST_INFRA}/docker-compose.yml" ]; then
    pass "docker-compose.yml exists"
  else
    fail "docker-compose.yml exists" "File not found at ${TEST_INFRA}/docker-compose.yml"
  fi

  # seed.sql exists
  if [ -f "${TEST_INFRA}/seed.sql" ]; then
    pass "seed.sql exists"
  else
    fail "seed.sql exists" "File not found at ${TEST_INFRA}/seed.sql"
  fi

  # wait-for-deps.sh exists and is executable
  if [ -f "${TEST_INFRA}/wait-for-deps.sh" ]; then
    pass "wait-for-deps.sh exists"
    if [ -x "${TEST_INFRA}/wait-for-deps.sh" ]; then
      pass "wait-for-deps.sh is executable"
    else
      fail "wait-for-deps.sh is executable" "Missing execute permission"
    fi
  else
    fail "wait-for-deps.sh exists" "File not found"
    fail "wait-for-deps.sh is executable" "File not found"
  fi

  section "docker-compose.yml structure"

  if [ -f "${TEST_INFRA}/docker-compose.yml" ]; then
    local compose="${TEST_INFRA}/docker-compose.yml"

    # Postgres service
    if grep -q 'postgres:' "$compose" 2>/dev/null; then
      pass "docker-compose defines postgres service"
    else
      fail "docker-compose defines postgres service"
    fi

    # Postgres 16 image
    if grep -q 'postgres:16' "$compose" 2>/dev/null; then
      pass "postgres uses version 16"
    else
      fail "postgres uses version 16"
    fi

    # Redis service
    if grep -q 'redis:' "$compose" 2>/dev/null; then
      pass "docker-compose defines redis service"
    else
      fail "docker-compose defines redis service"
    fi

    # Redis 7 image
    if grep -q 'redis:7' "$compose" 2>/dev/null; then
      pass "redis uses version 7"
    else
      fail "redis uses version 7"
    fi

    # Mock-registry service
    if grep -q 'mock-registry:' "$compose" 2>/dev/null; then
      pass "docker-compose defines mock-registry service"
    else
      fail "docker-compose defines mock-registry service"
    fi

    # tmpfs for postgres data
    if grep -q 'tmpfs' "$compose" 2>/dev/null; then
      pass "tmpfs configured for data directories"
    else
      fail "tmpfs configured for data directories"
    fi

    # seed.sql mounted/referenced
    if grep -q 'seed.sql' "$compose" 2>/dev/null; then
      pass "seed.sql referenced in compose file"
    else
      fail "seed.sql referenced in compose file"
    fi

    # Port mappings for postgres
    if grep -qE '5432' "$compose" 2>/dev/null; then
      pass "postgres port 5432 exposed"
    else
      fail "postgres port 5432 exposed"
    fi

    # Port mappings for redis
    if grep -qE '6379' "$compose" 2>/dev/null; then
      pass "redis port 6379 exposed"
    else
      fail "redis port 6379 exposed"
    fi

    # Health checks defined
    if grep -q 'healthcheck' "$compose" 2>/dev/null; then
      pass "healthchecks defined in compose"
    else
      fail "healthchecks defined in compose"
    fi
  else
    for t in "docker-compose defines postgres service" \
             "postgres uses version 16" \
             "docker-compose defines redis service" \
             "redis uses version 7" \
             "docker-compose defines mock-registry service" \
             "tmpfs configured for data directories" \
             "seed.sql referenced in compose file" \
             "postgres port 5432 exposed" \
             "redis port 6379 exposed" \
             "healthchecks defined in compose"; do
      fail "$t" "docker-compose.yml not found"
    done
  fi

  section "seed.sql content"

  if [ -f "${TEST_INFRA}/seed.sql" ]; then
    local seed="${TEST_INFRA}/seed.sql"

    # CREATE TABLE test_users
    if grep -qi 'CREATE TABLE.*test_users' "$seed" 2>/dev/null; then
      pass "seed.sql creates test_users table"
    else
      fail "seed.sql creates test_users table"
    fi

    # 5 rows in test_users (count INSERT statements or VALUES rows)
    local insert_count
    insert_count=$(grep -ci "INSERT INTO.*test_users" "$seed" 2>/dev/null || echo "0")
    # Also count VALUES in a multi-row insert
    local values_count
    values_count=$(grep -c "VALUES\|, *(" "$seed" 2>/dev/null || echo "0")

    # Check that we have seed data (at least 5 entries in some form)
    if grep -qE "INSERT INTO.*test_users" "$seed" 2>/dev/null; then
      pass "seed.sql inserts data into test_users"
      # Verify 5 rows by counting value tuples
      local row_count
      row_count=$(grep -cE "^\s*\(" "$seed" 2>/dev/null || echo "0")
      if [ "$row_count" -ge 5 ]; then
        pass "seed.sql has at least 5 rows for test_users"
      else
        # Try counting commas in a multi-row VALUES
        local tuple_count
        tuple_count=$(grep -oE "\([^)]+\)" "$seed" | grep -c "'" 2>/dev/null || echo "0")
        if [ "$tuple_count" -ge 5 ]; then
          pass "seed.sql has at least 5 rows for test_users"
        else
          fail "seed.sql has at least 5 rows for test_users" "Found fewer than 5 value tuples"
        fi
      fi
    else
      fail "seed.sql inserts data into test_users"
      fail "seed.sql has at least 5 rows for test_users" "No INSERT statements found"
    fi

    # CREATE TABLE test_analytics
    if grep -qi 'CREATE TABLE.*test_analytics' "$seed" 2>/dev/null; then
      pass "seed.sql creates test_analytics table"
    else
      fail "seed.sql creates test_analytics table"
    fi
  else
    for t in "seed.sql creates test_users table" \
             "seed.sql inserts data into test_users" \
             "seed.sql has at least 5 rows for test_users" \
             "seed.sql creates test_analytics table"; do
      fail "$t" "seed.sql not found"
    done
  fi

  section "wait-for-deps.sh structure"

  if [ -f "${TEST_INFRA}/wait-for-deps.sh" ]; then
    local wait="${TEST_INFRA}/wait-for-deps.sh"

    # Has shebang
    if head -1 "$wait" | grep -q '#!/usr/bin/env bash\|#!/bin/bash'; then
      pass "wait-for-deps.sh has bash shebang"
    else
      fail "wait-for-deps.sh has bash shebang"
    fi

    # Checks postgres
    if grep -q 'pg_isready\|psql\|5432' "$wait" 2>/dev/null; then
      pass "wait-for-deps.sh checks postgres readiness"
    else
      fail "wait-for-deps.sh checks postgres readiness"
    fi

    # Checks redis
    if grep -q 'redis-cli\|PING\|6379' "$wait" 2>/dev/null; then
      pass "wait-for-deps.sh checks redis readiness"
    else
      fail "wait-for-deps.sh checks redis readiness"
    fi

    # Checks mock-registry
    if grep -q 'mock-registry\|8888\|curl\|wget' "$wait" 2>/dev/null; then
      pass "wait-for-deps.sh checks mock-registry readiness"
    else
      fail "wait-for-deps.sh checks mock-registry readiness"
    fi

    # Has timeout logic (~60 seconds)
    if grep -qE '60|TIMEOUT|timeout' "$wait" 2>/dev/null; then
      pass "wait-for-deps.sh has 60s timeout"
    else
      fail "wait-for-deps.sh has 60s timeout"
    fi

    # Exits with code 1 on timeout
    if grep -qE 'exit 1' "$wait" 2>/dev/null; then
      pass "wait-for-deps.sh exits 1 on timeout"
    else
      fail "wait-for-deps.sh exits 1 on timeout"
    fi
  else
    for t in "wait-for-deps.sh has bash shebang" \
             "wait-for-deps.sh checks postgres readiness" \
             "wait-for-deps.sh checks redis readiness" \
             "wait-for-deps.sh checks mock-registry readiness" \
             "wait-for-deps.sh has 60s timeout" \
             "wait-for-deps.sh exits 1 on timeout"; do
      fail "$t" "wait-for-deps.sh not found"
    done
  fi

  section "mock-registry"

  # Check mock-registry has some implementation
  if [ -f "${TEST_INFRA}/mock-registry/server.js" ] || \
     [ -f "${TEST_INFRA}/mock-registry/Dockerfile" ] || \
     [ -f "${TEST_INFRA}/mock-registry.py" ] || \
     grep -q 'mock-registry' "${TEST_INFRA}/docker-compose.yml" 2>/dev/null; then
    pass "mock-registry implementation exists"
  else
    fail "mock-registry implementation exists"
  fi
}

# ─── Integration Tests: Docker lifecycle ───────────────────────────────────────

test_integration() {
  section "Prerequisites"

  # Check Docker is available
  if ! command -v docker &>/dev/null; then
    skip "Docker available" "docker not found in PATH"
    skip "Docker Compose available" "docker not found"
    skip "Full lifecycle test" "Docker required"
    return
  fi
  pass "Docker available"

  if ! docker compose version &>/dev/null 2>&1; then
    skip "Docker Compose available" "docker compose not available"
    skip "Full lifecycle test" "Docker Compose required"
    return
  fi
  pass "Docker Compose available"

  # Check Docker daemon is running
  if ! docker info &>/dev/null 2>&1; then
    skip "Docker daemon running" "Cannot connect to Docker daemon"
    skip "Full lifecycle test" "Docker daemon required"
    return
  fi
  pass "Docker daemon running"

  section "Full lifecycle test"

  local compose_file="${TEST_INFRA}/docker-compose.yml"
  if [ ! -f "$compose_file" ]; then
    fail "Full lifecycle test" "docker-compose.yml not found"
    return
  fi

  # Ensure clean state
  docker compose -f "$compose_file" -p warpgrid-test down --volumes --remove-orphans 2>/dev/null || true

  # Bring up
  printf "  … Starting services…\n"
  if docker compose -f "$compose_file" -p warpgrid-test up -d 2>/dev/null; then
    pass "docker compose up succeeds"
  else
    fail "docker compose up succeeds" "docker compose up failed"
    docker compose -f "$compose_file" -p warpgrid-test down --volumes 2>/dev/null || true
    return
  fi

  # Wait for deps
  printf "  … Waiting for services to be ready…\n"
  if "${TEST_INFRA}/wait-for-deps.sh" 2>/dev/null; then
    pass "wait-for-deps.sh exits 0 (all services ready)"
  else
    fail "wait-for-deps.sh exits 0 (all services ready)" "Script exited non-zero"
    docker compose -f "$compose_file" -p warpgrid-test logs 2>/dev/null || true
    docker compose -f "$compose_file" -p warpgrid-test down --volumes 2>/dev/null || true
    return
  fi

  # Test Postgres: SELECT 1
  printf "  … Testing Postgres connectivity…\n"
  local pg_result
  pg_result=$(docker compose -f "$compose_file" -p warpgrid-test exec -T postgres \
    psql -U warpgrid -d warpgrid_test -tAc "SELECT 1" 2>/dev/null || echo "FAIL")
  if [ "$pg_result" = "1" ]; then
    pass "Postgres SELECT 1 returns 1"
  else
    fail "Postgres SELECT 1 returns 1" "Got: ${pg_result}"
  fi

  # Test Postgres: seed data loaded
  local user_count
  user_count=$(docker compose -f "$compose_file" -p warpgrid-test exec -T postgres \
    psql -U warpgrid -d warpgrid_test -tAc "SELECT count(*) FROM test_users" 2>/dev/null || echo "0")
  if [ "$user_count" = "5" ]; then
    pass "test_users has 5 seed rows"
  else
    fail "test_users has 5 seed rows" "Got count: ${user_count}"
  fi

  # Test Postgres: test_analytics table exists
  local analytics_exists
  analytics_exists=$(docker compose -f "$compose_file" -p warpgrid-test exec -T postgres \
    psql -U warpgrid -d warpgrid_test -tAc "SELECT count(*) FROM information_schema.tables WHERE table_name = 'test_analytics'" 2>/dev/null || echo "0")
  if [ "$analytics_exists" = "1" ]; then
    pass "test_analytics table exists"
  else
    fail "test_analytics table exists" "Table not found"
  fi

  # Test Redis: PING
  printf "  … Testing Redis connectivity…\n"
  local redis_result
  redis_result=$(docker compose -f "$compose_file" -p warpgrid-test exec -T redis \
    redis-cli PING 2>/dev/null || echo "FAIL")
  if [ "$redis_result" = "PONG" ]; then
    pass "Redis PING returns PONG"
  else
    fail "Redis PING returns PONG" "Got: ${redis_result}"
  fi

  # Test mock-registry: health check
  printf "  … Testing mock-registry connectivity…\n"
  local registry_result
  registry_result=$(docker compose -f "$compose_file" -p warpgrid-test exec -T mock-registry \
    wget -qO- http://localhost:8888/health 2>/dev/null || \
    docker compose -f "$compose_file" -p warpgrid-test exec -T mock-registry \
    curl -sf http://localhost:8888/health 2>/dev/null || echo "FAIL")
  if echo "$registry_result" | grep -qi 'ok\|healthy\|alive'; then
    pass "mock-registry /health returns ok"
  else
    fail "mock-registry /health returns ok" "Got: ${registry_result}"
  fi

  # Test mock-registry: service discovery endpoint
  local discovery_result
  discovery_result=$(docker compose -f "$compose_file" -p warpgrid-test exec -T mock-registry \
    wget -qO- http://localhost:8888/services 2>/dev/null || \
    docker compose -f "$compose_file" -p warpgrid-test exec -T mock-registry \
    curl -sf http://localhost:8888/services 2>/dev/null || echo "FAIL")
  if echo "$discovery_result" | grep -qi 'warp.local\|services'; then
    pass "mock-registry /services returns discovery data"
  else
    fail "mock-registry /services returns discovery data" "Got: ${discovery_result}"
  fi

  # Tear down
  printf "  … Tearing down…\n"
  if docker compose -f "$compose_file" -p warpgrid-test down --volumes --remove-orphans 2>/dev/null; then
    pass "docker compose down succeeds"
  else
    fail "docker compose down succeeds"
  fi
}

# ─── Main ──────────────────────────────────────────────────────────────────────

main() {
  local mode="${1:---unit}"

  printf "${BLUE}US-701: Docker Compose Test Dependency Stack${NC}\n"
  printf "═══════════════════════════════════════════════\n"

  case "$mode" in
    --unit)
      test_unit
      ;;
    --integration)
      test_unit
      test_integration
      ;;
    *)
      test_unit
      test_integration
      ;;
  esac

  # Summary
  printf "\n═══════════════════════════════════════════════\n"
  printf "Total: %d  " "$TOTAL"
  printf "${GREEN}Passed: %d${NC}  " "$PASSED"
  if [ "$FAILED" -gt 0 ]; then
    printf "${RED}Failed: %d${NC}  " "$FAILED"
  else
    printf "Failed: %d  " "$FAILED"
  fi
  if [ "$SKIPPED" -gt 0 ]; then
    printf "${YELLOW}Skipped: %d${NC}" "$SKIPPED"
  else
    printf "Skipped: %d" "$SKIPPED"
  fi
  printf "\n"

  if [ "$FAILED" -gt 0 ]; then
    exit 1
  fi
  exit 0
}

main "$@"
