#!/usr/bin/env bash
#
# wait-for-deps.sh — Poll test dependency services until all are ready.
#
# Checks:
#   - Postgres (pg_isready on port 5432)
#   - Redis (redis-cli PING on port 6379)
#   - Mock-registry (HTTP /health on port 8888)
#
# Usage:
#   test-infra/wait-for-deps.sh              # Wait with defaults
#   TIMEOUT=30 test-infra/wait-for-deps.sh   # Custom timeout
#
# Environment:
#   TIMEOUT        Max wait time in seconds (default: 60)
#   POSTGRES_HOST  Postgres host (default: localhost)
#   POSTGRES_PORT  Postgres port (default: 5432)
#   REDIS_HOST     Redis host (default: localhost)
#   REDIS_PORT     Redis port (default: 6379)
#   REGISTRY_HOST  Mock-registry host (default: localhost)
#   REGISTRY_PORT  Mock-registry port (default: 8888)
#
# Exit codes:
#   0  All services ready
#   1  Timeout — one or more services not ready within TIMEOUT seconds

set -euo pipefail

TIMEOUT="${TIMEOUT:-60}"
POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
REDIS_HOST="${REDIS_HOST:-localhost}"
REDIS_PORT="${REDIS_PORT:-6379}"
REGISTRY_HOST="${REGISTRY_HOST:-localhost}"
REGISTRY_PORT="${REGISTRY_PORT:-8888}"

# Track readiness
pg_ready=false
redis_ready=false
registry_ready=false

elapsed=0
interval=2

echo "Waiting for test dependencies (timeout: ${TIMEOUT}s)..."

while [ "$elapsed" -lt "$TIMEOUT" ]; do
  # Check Postgres
  if [ "$pg_ready" = false ]; then
    if pg_isready -h "$POSTGRES_HOST" -p "$POSTGRES_PORT" -U warpgrid -q 2>/dev/null; then
      pg_ready=true
      echo "  ✓ Postgres ready (${elapsed}s)"
    fi
  fi

  # Check Redis
  if [ "$redis_ready" = false ]; then
    if redis-cli -h "$REDIS_HOST" -p "$REDIS_PORT" ping 2>/dev/null | grep -q PONG; then
      redis_ready=true
      echo "  ✓ Redis ready (${elapsed}s)"
    fi
  fi

  # Check mock-registry
  if [ "$registry_ready" = false ]; then
    if curl -sf "http://${REGISTRY_HOST}:${REGISTRY_PORT}/health" >/dev/null 2>&1; then
      registry_ready=true
      echo "  ✓ Mock-registry ready (${elapsed}s)"
    elif wget -qO- "http://${REGISTRY_HOST}:${REGISTRY_PORT}/health" >/dev/null 2>&1; then
      registry_ready=true
      echo "  ✓ Mock-registry ready (${elapsed}s)"
    fi
  fi

  # All ready?
  if [ "$pg_ready" = true ] && [ "$redis_ready" = true ] && [ "$registry_ready" = true ]; then
    echo "All services ready in ${elapsed}s."
    exit 0
  fi

  sleep "$interval"
  elapsed=$((elapsed + interval))
done

# Timeout
echo "ERROR: Timed out after ${TIMEOUT}s waiting for services."
[ "$pg_ready" = false ]       && echo "  ✗ Postgres not ready"
[ "$redis_ready" = false ]    && echo "  ✗ Redis not ready"
[ "$registry_ready" = false ] && echo "  ✗ Mock-registry not ready"
exit 1
