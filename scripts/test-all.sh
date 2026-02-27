#!/usr/bin/env bash
#
# test-all.sh — Orchestrate the full WarpGrid integration test lifecycle.
#
# Discovers test apps in test-apps/t*-*/, builds each via build.sh (if present),
# runs each via test.sh (if present), collects logs to test-results/, and
# prints a summary table.
#
# Usage:
#   ./test-all.sh                  Run all discovered test apps
#   ./test-all.sh --only t3,t5     Run only specified test apps
#   ./test-all.sh --keep-deps      Skip Docker Compose teardown after tests
#   ./test-all.sh --verbose        Show full build/test output in real-time
#   ./test-all.sh --dry-run        Print execution plan without running
#
# Prerequisites:
#   - curl (for HTTP tests)
#   - docker / docker compose (for dependency services, optional)
#   - Language-specific tools: go, bun, jco (per test app)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TEST_APPS_DIR="${PROJECT_ROOT}/test-apps"
RESULTS_DIR="${PROJECT_ROOT}/test-results"
TEST_INFRA_DIR="${PROJECT_ROOT}/test-infra"
TIMESTAMP="$(date +%Y%m%d-%H%M%S)"

# ── Default flags ────────────────────────────────────────────────────

DRY_RUN=false
VERBOSE=false
KEEP_DEPS=false
ONLY_FILTER=""

# ── Parse args ───────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)   DRY_RUN=true ;;
        --verbose)   VERBOSE=true ;;
        --keep-deps) KEEP_DEPS=true ;;
        --only)
            shift
            if [ $# -eq 0 ]; then
                echo "ERROR: --only requires a comma-separated list of test apps" >&2
                exit 2
            fi
            ONLY_FILTER="$1"
            ;;
        --only=*)
            ONLY_FILTER="${1#--only=}"
            ;;
        --help|-h)
            cat <<'USAGE'
Usage: test-all.sh [OPTIONS]

Orchestrate the full WarpGrid integration test lifecycle.

Options:
  --only t1,t3     Run only the specified test apps (comma-separated)
  --keep-deps      Skip Docker Compose teardown after tests
  --verbose        Show full build/test output in real-time
  --dry-run        Print execution plan without running anything
  --help, -h       Show this help message

Examples:
  ./test-all.sh                    Run all test apps
  ./test-all.sh --only t3,t5       Run T3 and T5 only
  ./test-all.sh --verbose          Full output mode
  ./test-all.sh --dry-run          Preview what would run
USAGE
            exit 0
            ;;
        *)
            echo "ERROR: Unknown flag: $1" >&2
            echo "Run test-all.sh --help for usage." >&2
            exit 2
            ;;
    esac
    shift
done

# ── Helpers ──────────────────────────────────────────────────────────

log()   { echo "==> $*" >&2; }
info()  { echo "    $*" >&2; }
warn()  { echo "  WARN: $*" >&2; }

# Extract short name (e.g. "t3") from directory name (e.g. "t3-go-http-postgres")
app_short_name() {
    local dir_name
    dir_name="$(basename "$1")"
    echo "${dir_name}" | sed 's/-.*//'
}

# Check if a short name is in the --only filter
is_selected() {
    local name="$1"
    if [ -z "${ONLY_FILTER}" ]; then
        return 0  # no filter = all selected
    fi
    # Split on commas and check each
    local IFS=","
    local item
    for item in ${ONLY_FILTER}; do
        if [ "${item}" = "${name}" ]; then
            return 0
        fi
    done
    return 1
}

# ── Discover test apps ───────────────────────────────────────────────

declare -a APP_DIRS=()
declare -a APP_NAMES=()
declare -a APP_HAS_BUILD=()
declare -a APP_HAS_TEST=()

if [ -d "${TEST_APPS_DIR}" ]; then
    for app_dir in "${TEST_APPS_DIR}"/t*-*/; do
        [ -d "${app_dir}" ] || continue
        short=$(app_short_name "${app_dir}")
        if ! is_selected "${short}"; then
            continue
        fi
        APP_DIRS+=("${app_dir}")
        APP_NAMES+=("${short}")
        if [ -x "${app_dir}build.sh" ]; then
            APP_HAS_BUILD+=("true")
        else
            APP_HAS_BUILD+=("false")
        fi
        if [ -x "${app_dir}test.sh" ]; then
            APP_HAS_TEST+=("true")
        else
            APP_HAS_TEST+=("false")
        fi
    done
fi

APP_COUNT=${#APP_DIRS[@]}

# ── Check prerequisites ─────────────────────────────────────────────

HAVE_DOCKER=false
HAVE_DOCKER_COMPOSE=false
HAVE_CURL=false
HAVE_GO=false
HAVE_BUN=false

command -v docker &>/dev/null && HAVE_DOCKER=true
(command -v docker &>/dev/null && docker compose version &>/dev/null) && HAVE_DOCKER_COMPOSE=true
command -v curl &>/dev/null && HAVE_CURL=true
command -v go &>/dev/null && HAVE_GO=true
command -v bun &>/dev/null && HAVE_BUN=true

HAVE_JCO=false
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"
[ -x "${JCO_BIN}" ] && HAVE_JCO=true

HAVE_TEST_INFRA=false
[ -f "${TEST_INFRA_DIR}/docker-compose.yml" ] && HAVE_TEST_INFRA=true

# ── Dry-run: print execution plan and exit ───────────────────────────

if ${DRY_RUN}; then
    echo ""
    echo "════════════════════════════════════════════════════════════"
    echo "  Execution Plan: WarpGrid Integration Tests"
    echo "════════════════════════════════════════════════════════════"
    echo ""

    echo "Prerequisites available:"
    echo "  docker:          $(${HAVE_DOCKER} && echo 'yes' || echo 'no')"
    echo "  docker compose:  $(${HAVE_DOCKER_COMPOSE} && echo 'yes' || echo 'no')"
    echo "  curl:            $(${HAVE_CURL} && echo 'yes' || echo 'no')"
    echo "  go:              $(${HAVE_GO} && echo 'yes' || echo 'no')"
    echo "  bun:             $(${HAVE_BUN} && echo 'yes' || echo 'no')"
    echo "  jco:             $(${HAVE_JCO} && echo 'yes' || echo 'no')"
    echo ""

    if ${HAVE_TEST_INFRA}; then
        echo "Docker Compose:    ${TEST_INFRA_DIR}/docker-compose.yml"
    else
        echo "Docker Compose:    not available (test-infra/ not found)"
    fi
    echo ""

    echo "Flags:"
    echo "  --only:          ${ONLY_FILTER:-'(all)'}"
    echo "  --verbose:       ${VERBOSE}"
    echo "  --keep-deps:     ${KEEP_DEPS}"
    echo ""

    if [ "${APP_COUNT}" -eq 0 ]; then
        echo "No test apps discovered."
        exit 0
    fi

    echo "Test apps (${APP_COUNT}):"
    echo ""
    printf "  %-6s  %-30s  %-8s  %-8s  %s\n" "NAME" "DIRECTORY" "BUILD" "TEST" "STATUS"
    printf "  %-6s  %-30s  %-8s  %-8s  %s\n" "------" "------------------------------" "--------" "--------" "--------"

    for i in $(seq 0 $((APP_COUNT - 1))); do
        local_name="${APP_NAMES[$i]}"
        local_dir="$(basename "${APP_DIRS[$i]}")"
        local_build="${APP_HAS_BUILD[$i]}"
        local_test="${APP_HAS_TEST[$i]}"

        status="RUN"
        if [ "${local_test}" = "false" ]; then
            status="SKIP (no test.sh)"
        fi

        printf "  %-6s  %-30s  %-8s  %-8s  %s\n" \
            "${local_name}" "${local_dir}" "${local_build}" "${local_test}" "${status}"
    done

    echo ""
    echo "Execution order:"
    echo "  1. Validate prerequisites"
    if ${HAVE_TEST_INFRA}; then
        echo "  2. Start Docker Compose (test-infra/)"
        echo "  3. Wait for dependencies"
    else
        echo "  2. (skip Docker Compose — test-infra/ not found)"
    fi
    echo "  4. Build test apps (parallel where independent)"
    echo "  5. Run test suites"
    echo "  6. Print summary"
    if ! ${KEEP_DEPS}; then
        echo "  7. Tear down Docker Compose"
    else
        echo "  7. (skip teardown — --keep-deps)"
    fi
    echo ""
    exit 0
fi

# ── Execution mode ───────────────────────────────────────────────────

log "WarpGrid Integration Test Suite"
log "Timestamp: ${TIMESTAMP}"
echo ""

if [ "${APP_COUNT}" -eq 0 ]; then
    log "No test apps discovered in ${TEST_APPS_DIR}"
    exit 0
fi

# Create results directory
mkdir -p "${RESULTS_DIR}"

# Track overall results
declare -a RESULT_STATUS=()  # PASS, FAIL, SKIP, BUILD_FAIL
declare -a RESULT_DETAIL=()

# ── Step 1: Prerequisites ────────────────────────────────────────────

log "Step 1: Checking prerequisites"

if ! ${HAVE_CURL}; then
    warn "curl not found — HTTP integration tests will fail"
fi

# ── Step 2: Docker Compose (if available) ────────────────────────────

COMPOSE_STARTED=false

if ${HAVE_TEST_INFRA} && ${HAVE_DOCKER_COMPOSE}; then
    log "Step 2: Starting Docker Compose services"

    COMPOSE_LOG="${RESULTS_DIR}/${TIMESTAMP}-docker-compose.log"

    if ${VERBOSE}; then
        docker compose -f "${TEST_INFRA_DIR}/docker-compose.yml" up -d 2>&1 | tee "${COMPOSE_LOG}"
    else
        docker compose -f "${TEST_INFRA_DIR}/docker-compose.yml" up -d >"${COMPOSE_LOG}" 2>&1 || true
    fi

    COMPOSE_STARTED=true

    # Wait for deps if script exists
    if [ -x "${TEST_INFRA_DIR}/wait-for-deps.sh" ]; then
        log "Waiting for dependency services..."
        if ${VERBOSE}; then
            "${TEST_INFRA_DIR}/wait-for-deps.sh" 2>&1 | tee -a "${COMPOSE_LOG}" || true
        else
            "${TEST_INFRA_DIR}/wait-for-deps.sh" >>"${COMPOSE_LOG}" 2>&1 || true
        fi
    fi
else
    log "Step 2: Skipping Docker Compose ($(${HAVE_TEST_INFRA} || echo 'no test-infra/'; ${HAVE_DOCKER_COMPOSE} || echo 'no docker compose'))"
fi

# ── Step 3: Build test apps ──────────────────────────────────────────

log "Step 3: Building test apps"

declare -a BUILD_PIDS=()
declare -a BUILD_LOGS=()
declare -a BUILD_NAMES=()

for i in $(seq 0 $((APP_COUNT - 1))); do
    name="${APP_NAMES[$i]}"
    dir="${APP_DIRS[$i]}"
    has_build="${APP_HAS_BUILD[$i]}"

    BUILD_LOG="${RESULTS_DIR}/${TIMESTAMP}-${name}-build.log"
    BUILD_LOGS+=("${BUILD_LOG}")
    BUILD_NAMES+=("${name}")

    if [ "${has_build}" = "false" ]; then
        echo "SKIP: no build.sh" > "${BUILD_LOG}"
        BUILD_PIDS+=("0")  # sentinel for skip
        info "${name}: no build.sh (skip build)"
        continue
    fi

    info "${name}: building..."

    # Run builds in background for parallelism
    (
        cd "${dir}"
        if ./build.sh --standalone >"${BUILD_LOG}" 2>&1; then
            echo "BUILD_OK" >> "${BUILD_LOG}"
        else
            echo "BUILD_FAIL" >> "${BUILD_LOG}"
        fi
    ) &
    BUILD_PIDS+=("$!")
done

# Wait for all builds to complete
for i in $(seq 0 $((${#BUILD_PIDS[@]} - 1))); do
    pid="${BUILD_PIDS[$i]}"
    name="${BUILD_NAMES[$i]}"

    if [ "${pid}" = "0" ]; then
        continue  # was a skip
    fi

    if wait "${pid}" 2>/dev/null; then
        info "${name}: build complete"
    else
        info "${name}: build may have issues (check log)"
    fi
done

# Check build results
declare -a BUILD_OK=()
for i in $(seq 0 $((APP_COUNT - 1))); do
    log_file="${BUILD_LOGS[$i]}"
    if grep -q "BUILD_OK" "${log_file}" 2>/dev/null; then
        BUILD_OK+=("true")
    elif grep -q "SKIP" "${log_file}" 2>/dev/null; then
        BUILD_OK+=("skip")
    else
        BUILD_OK+=("false")
    fi
done

# ── Step 4: Run test suites ──────────────────────────────────────────

log "Step 4: Running test suites"

for i in $(seq 0 $((APP_COUNT - 1))); do
    name="${APP_NAMES[$i]}"
    dir="${APP_DIRS[$i]}"
    has_test="${APP_HAS_TEST[$i]}"
    build_ok="${BUILD_OK[$i]}"

    TEST_LOG="${RESULTS_DIR}/${TIMESTAMP}-${name}-test.log"

    # Skip if no test.sh
    if [ "${has_test}" = "false" ]; then
        info "${name}: SKIP (no test.sh)"
        RESULT_STATUS+=("SKIP")
        RESULT_DETAIL+=("no test.sh")
        echo "SKIP: no test.sh" > "${TEST_LOG}"
        continue
    fi

    # Skip if build failed
    if [ "${build_ok}" = "false" ]; then
        info "${name}: SKIP (build failed)"
        RESULT_STATUS+=("SKIP")
        RESULT_DETAIL+=("build failed")
        echo "SKIP: build failed" > "${TEST_LOG}"
        continue
    fi

    info "${name}: running tests..."

    if ${VERBOSE}; then
        (
            cd "${dir}"
            ./test.sh 2>&1 | tee "${TEST_LOG}"
        )
        TEST_EXIT=$?
    else
        (
            cd "${dir}"
            ./test.sh >"${TEST_LOG}" 2>&1
        )
        TEST_EXIT=$?
    fi

    if [ ${TEST_EXIT} -eq 0 ]; then
        RESULT_STATUS+=("PASS")
        # Extract result line from log
        RESULT_LINE=$(grep -E "^Results:" "${TEST_LOG}" 2>/dev/null | tail -1 || echo "")
        RESULT_DETAIL+=("${RESULT_LINE:-passed}")
        info "${name}: PASS"
    else
        RESULT_STATUS+=("FAIL")
        RESULT_LINE=$(grep -E "^Results:" "${TEST_LOG}" 2>/dev/null | tail -1 || echo "")
        RESULT_DETAIL+=("${RESULT_LINE:-exit code ${TEST_EXIT}}")
        info "${name}: FAIL"
    fi
done

# ── Step 5: Summary table ────────────────────────────────────────────

echo ""
echo "════════════════════════════════════════════════════════════"
echo "  Summary: WarpGrid Integration Tests (${TIMESTAMP})"
echo "════════════════════════════════════════════════════════════"
echo ""

printf "  %-6s  %-8s  %s\n" "APP" "STATUS" "DETAILS"
printf "  %-6s  %-8s  %s\n" "------" "--------" "-------------------------------------------"

TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0

for i in $(seq 0 $((APP_COUNT - 1))); do
    name="${APP_NAMES[$i]}"
    status="${RESULT_STATUS[$i]}"
    detail="${RESULT_DETAIL[$i]}"

    case "${status}" in
        PASS) TOTAL_PASS=$((TOTAL_PASS + 1)) ;;
        FAIL) TOTAL_FAIL=$((TOTAL_FAIL + 1)) ;;
        SKIP) TOTAL_SKIP=$((TOTAL_SKIP + 1)) ;;
    esac

    printf "  %-6s  %-8s  %s\n" "${name}" "${status}" "${detail}"
done

echo ""
echo "  Total: ${TOTAL_PASS} passed, ${TOTAL_FAIL} failed, ${TOTAL_SKIP} skipped"
echo ""
echo "  Logs: ${RESULTS_DIR}/${TIMESTAMP}-*.log"
echo ""

# ── Step 6: Teardown ─────────────────────────────────────────────────

if ${COMPOSE_STARTED} && ! ${KEEP_DEPS}; then
    log "Step 6: Tearing down Docker Compose"
    docker compose -f "${TEST_INFRA_DIR}/docker-compose.yml" down >"${RESULTS_DIR}/${TIMESTAMP}-teardown.log" 2>&1 || true
elif ${COMPOSE_STARTED} && ${KEEP_DEPS}; then
    log "Step 6: Skipping teardown (--keep-deps)"
fi

# ── Exit code ─────────────────────────────────────────────────────────

if [ ${TOTAL_FAIL} -gt 0 ]; then
    exit 1
fi
exit 0
