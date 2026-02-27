#!/usr/bin/env bash
#
# test-bun-compat.sh — Test Bun ecosystem packages for Wasm compatibility
#
# Tests each package in compat-db/bun/packages.json through the
# `warp pack --lang bun` pipeline:
#   1. bun install (can the package be installed?)
#   2. bun build --target browser --format esm (can it bundle?)
#   3. jco componentize (can it become a WASI component?)
#
# Results are written to compat-db/bun/results.json.
#
# Usage:
#   scripts/test-bun-compat.sh                    # Test all packages
#   scripts/test-bun-compat.sh --only hono,zod    # Test specific packages
#   scripts/test-bun-compat.sh --json             # Only output JSON (no progress)
#   scripts/test-bun-compat.sh --help             # Show help
#
# Prerequisites:
#   - bun (https://bun.sh)
#   - jco (via scripts/build-componentize-js.sh)
#   - wasm-tools (optional, for validation)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
COMPAT_DIR="${PROJECT_ROOT}/compat-db/bun"
PACKAGES_JSON="${COMPAT_DIR}/packages.json"
RESULTS_JSON="${COMPAT_DIR}/results.json"
JCO_BIN="${PROJECT_ROOT}/build/componentize-js/node_modules/.bin/jco"
WIT_DIR="${PROJECT_ROOT}/tests/fixtures/js-http-handler/wit"
WORK_DIR=""
RESULTS_DIR=""

# Options
ONLY_PACKAGES=""
JSON_ONLY=false

# ── Helpers ────────────────────────────────────────────────────────────

log() {
    if [ "${JSON_ONLY}" = false ]; then
        echo "==> $*" >&2
    fi
}

err() { echo "ERROR: $*" >&2; exit 1; }

cleanup() {
    if [ -n "${WORK_DIR}" ] && [ -d "${WORK_DIR}" ]; then
        rm -rf "${WORK_DIR}"
    fi
    if [ -n "${RESULTS_DIR}" ] && [ -d "${RESULTS_DIR}" ]; then
        rm -rf "${RESULTS_DIR}"
    fi
}
trap cleanup EXIT

# Portable millisecond timestamp
now_ms() {
    if date +%s%3N 2>/dev/null | grep -qE '^[0-9]{13}'; then
        date +%s%3N
    else
        python3 -c 'import time; print(int(time.time()*1000))'
    fi
}

# ── Parse args ─────────────────────────────────────────────────────────

while [ $# -gt 0 ]; do
    case "$1" in
        --only)
            shift
            ONLY_PACKAGES="$1"
            ;;
        --json)
            JSON_ONLY=true
            ;;
        --help|-h)
            echo "Usage: test-bun-compat.sh [--only pkg1,pkg2] [--json]"
            echo ""
            echo "  --only pkg1,pkg2   Test only specified packages (comma-separated)"
            echo "  --json             Output only JSON results (no progress messages)"
            echo "  --help             Show this help"
            exit 0
            ;;
        *) err "Unknown flag: $1" ;;
    esac
    shift
done

# ── Prerequisite checks ───────────────────────────────────────────────

if ! command -v bun &>/dev/null; then
    err "bun not found. Install from https://bun.sh"
fi

if [ ! -f "${PACKAGES_JSON}" ]; then
    err "packages.json not found at ${PACKAGES_JSON}"
fi

if [ ! -d "${WIT_DIR}" ]; then
    err "WIT fixture not found at ${WIT_DIR}"
fi

# Set up jco if not available
if [ ! -x "${JCO_BIN}" ]; then
    log "jco not found. Building ComponentizeJS toolchain..."
    "${PROJECT_ROOT}/scripts/build-componentize-js.sh" --npm 2>&1 | tail -5
fi

if [ ! -x "${JCO_BIN}" ]; then
    err "jco still not found at ${JCO_BIN}. Run: scripts/build-componentize-js.sh"
fi

# Create working directories
WORK_DIR="$(mktemp -d)"
RESULTS_DIR="$(mktemp -d)"
log "Working directory: ${WORK_DIR}"

# ── Handler template generator ─────────────────────────────────────────

generate_handler() {
    local pkg_name="$1"
    local handler_file="$2"

    # Each package gets a handler that imports and exercises its core API.
    # Uses the web-standard fetch event pattern for componentization.
    case "${pkg_name}" in
        hono)
            cat > "${handler_file}" <<'HANDLER'
import { Hono } from "hono";
const app = new Hono();
app.get("/", (c) => c.json({ ok: true }));
addEventListener("fetch", (event) => event.respondWith(app.fetch(event.request)));
HANDLER
            ;;
        elysia)
            cat > "${handler_file}" <<'HANDLER'
import { Elysia } from "elysia";
const app = new Elysia().get("/", () => ({ ok: true }));
addEventListener("fetch", (event) => {
  event.respondWith(new Response(JSON.stringify({ framework: "elysia" }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        zod)
            cat > "${handler_file}" <<'HANDLER'
import { z } from "zod";
const schema = z.object({ name: z.string(), age: z.number().int().min(0) });
addEventListener("fetch", (event) => {
  const result = schema.safeParse({ name: "test", age: 25 });
  event.respondWith(new Response(JSON.stringify(result), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        drizzle-orm)
            cat > "${handler_file}" <<'HANDLER'
import { sql } from "drizzle-orm";
addEventListener("fetch", (event) => {
  const query = sql`SELECT 1`;
  event.respondWith(new Response(JSON.stringify({ imported: true }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        jose)
            cat > "${handler_file}" <<'HANDLER'
import { decodeJwt } from "jose";
addEventListener("fetch", (event) => {
  const token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
  try {
    const claims = decodeJwt(token);
    event.respondWith(new Response(JSON.stringify(claims), {
      headers: { "content-type": "application/json" }
    }));
  } catch (e) {
    event.respondWith(new Response(JSON.stringify({ error: String(e) }), { status: 500 }));
  }
});
HANDLER
            ;;
        nanoid)
            cat > "${handler_file}" <<'HANDLER'
import { nanoid } from "nanoid";
addEventListener("fetch", (event) => {
  const id = nanoid();
  event.respondWith(new Response(JSON.stringify({ id }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        date-fns)
            cat > "${handler_file}" <<'HANDLER'
import { format, parseISO } from "date-fns";
addEventListener("fetch", (event) => {
  const formatted = format(parseISO("2026-01-15"), "MMMM do, yyyy");
  event.respondWith(new Response(JSON.stringify({ date: formatted }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        lodash-es)
            cat > "${handler_file}" <<'HANDLER'
import { pick, merge, uniq } from "lodash-es";
addEventListener("fetch", (event) => {
  const obj = pick({ a: 1, b: 2, c: 3 }, ["a", "c"]);
  const merged = merge({}, obj, { d: 4 });
  const arr = uniq([1, 2, 2, 3]);
  event.respondWith(new Response(JSON.stringify({ obj: merged, arr }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        cheerio)
            cat > "${handler_file}" <<'HANDLER'
import * as cheerio from "cheerio";
addEventListener("fetch", (event) => {
  const $ = cheerio.load("<html><body><h1>Hello</h1></body></html>");
  const text = $("h1").text();
  event.respondWith(new Response(JSON.stringify({ title: text }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        superjson)
            cat > "${handler_file}" <<'HANDLER'
import superjson from "superjson";
addEventListener("fetch", (event) => {
  const data = { date: new Date("2026-01-01"), set: new Set([1, 2, 3]) };
  const serialized = superjson.stringify(data);
  const deserialized = superjson.parse(serialized);
  event.respondWith(new Response(serialized, {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        uuid)
            cat > "${handler_file}" <<'HANDLER'
import { v4 as uuidv4 } from "uuid";
addEventListener("fetch", (event) => {
  const id = uuidv4();
  event.respondWith(new Response(JSON.stringify({ id }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        ajv)
            cat > "${handler_file}" <<'HANDLER'
import Ajv from "ajv";
addEventListener("fetch", (event) => {
  const ajv = new Ajv();
  const schema = { type: "object", properties: { name: { type: "string" } }, required: ["name"] };
  const validate = ajv.compile(schema);
  const valid = validate({ name: "test" });
  event.respondWith(new Response(JSON.stringify({ valid }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        itty-router)
            cat > "${handler_file}" <<'HANDLER'
import { Router } from "itty-router";
const router = Router();
router.get("/", () => new Response(JSON.stringify({ ok: true }), {
  headers: { "content-type": "application/json" }
}));
addEventListener("fetch", (event) => event.respondWith(router.fetch(event.request)));
HANDLER
            ;;
        cookie)
            cat > "${handler_file}" <<'HANDLER'
import { parse, serialize } from "cookie";
addEventListener("fetch", (event) => {
  const cookies = parse("session=abc123; theme=dark");
  const header = serialize("new", "value", { httpOnly: true });
  event.respondWith(new Response(JSON.stringify({ cookies, header }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        ms)
            cat > "${handler_file}" <<'HANDLER'
import ms from "ms";
addEventListener("fetch", (event) => {
  const result = { "1h": ms("1h"), "2d": ms("2d"), pretty: ms(60000, { long: true }) };
  event.respondWith(new Response(JSON.stringify(result), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        mustache)
            cat > "${handler_file}" <<'HANDLER'
import Mustache from "mustache";
addEventListener("fetch", (event) => {
  const output = Mustache.render("Hello {{name}}!", { name: "WarpGrid" });
  event.respondWith(new Response(JSON.stringify({ output }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        marked)
            cat > "${handler_file}" <<'HANDLER'
import { marked } from "marked";
addEventListener("fetch", (event) => {
  const html = marked("# Hello\n\nWorld");
  event.respondWith(new Response(JSON.stringify({ html }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        devalue)
            cat > "${handler_file}" <<'HANDLER'
import { stringify, parse } from "devalue";
addEventListener("fetch", (event) => {
  const data = { date: new Date("2026-01-01"), re: /test/g };
  const encoded = stringify(data);
  event.respondWith(new Response(JSON.stringify({ encoded }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        "@sinclair/typebox")
            cat > "${handler_file}" <<'HANDLER'
import { Type } from "@sinclair/typebox";
import { Value } from "@sinclair/typebox/value";
addEventListener("fetch", (event) => {
  const T = Type.Object({ name: Type.String(), age: Type.Integer({ minimum: 0 }) });
  const ok = Value.Check(T, { name: "test", age: 25 });
  event.respondWith(new Response(JSON.stringify({ valid: ok }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        fast-json-stringify)
            cat > "${handler_file}" <<'HANDLER'
import fastJsonStringify from "fast-json-stringify";
addEventListener("fetch", (event) => {
  const stringify = fastJsonStringify({
    type: "object",
    properties: { name: { type: "string" }, age: { type: "integer" } }
  });
  const json = stringify({ name: "test", age: 25 });
  event.respondWith(new Response(json, {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
        *)
            # Fallback: just import and respond
            cat > "${handler_file}" <<HANDLER
import * as pkg from "${pkg_name}";
addEventListener("fetch", (event) => {
  event.respondWith(new Response(JSON.stringify({ imported: typeof pkg }), {
    headers: { "content-type": "application/json" }
  }));
});
HANDLER
            ;;
    esac
}

# ── Test a single package ──────────────────────────────────────────────

test_package() {
    local pkg_name="$1"
    local pkg_version="$2"
    local idx="$3"
    local pkg_dir="${WORK_DIR}/${pkg_name//\//_}"  # Replace / with _ for scoped packages
    local start_time
    local end_time
    local duration_ms
    local status="pass"
    local bundle_ok=false
    local componentize_ok=false
    local error_msg=""
    local error_stage=""
    local native_binding=false

    start_time=$(now_ms)

    mkdir -p "${pkg_dir}"

    # Step 1: Create package.json
    cat > "${pkg_dir}/package.json" <<EOF
{
  "name": "wg-compat-test-${pkg_name//\//-}",
  "version": "0.0.1",
  "private": true,
  "dependencies": {
    "${pkg_name}": "${pkg_version}"
  }
}
EOF

    # Step 2: Install
    log "  [${pkg_name}] Installing ${pkg_name}@${pkg_version}..."
    local install_output
    if ! install_output=$(cd "${pkg_dir}" && bun install --no-progress 2>&1); then
        status="fail-install"
        error_msg="${install_output}"
        error_stage="install"

        # Check for native binding failures
        if echo "${install_output}" | grep -qi "native\|binding\|gyp\|node-pre-gyp\|prebuild\|cmake\|make.*error"; then
            native_binding=true
        fi

        end_time=$(now_ms)
        duration_ms=$(( end_time - start_time ))
        write_result "${idx}" "${pkg_name}" "${pkg_version}" "${status}" "${bundle_ok}" "${componentize_ok}" "${error_msg}" "${error_stage}" "${native_binding}" "${duration_ms}"
        return
    fi

    # Step 3: Generate handler
    generate_handler "${pkg_name}" "${pkg_dir}/handler.js"

    # Step 4: Bundle with bun build
    log "  [${pkg_name}] Bundling..."
    local bundle_output
    local bundle_file="${pkg_dir}/bundle.js"
    if ! bundle_output=$(cd "${pkg_dir}" && bun build handler.js --outfile bundle.js --target browser --format esm 2>&1); then
        status="fail-build"
        error_msg="${bundle_output}"
        error_stage="bundle"

        if echo "${bundle_output}" | grep -qi "native\|binding\|Could not resolve\|No matching export"; then
            native_binding=true
        fi

        end_time=$(now_ms)
        duration_ms=$(( end_time - start_time ))
        write_result "${idx}" "${pkg_name}" "${pkg_version}" "${status}" "${bundle_ok}" "${componentize_ok}" "${error_msg}" "${error_stage}" "${native_binding}" "${duration_ms}"
        return
    fi

    if [ ! -f "${bundle_file}" ]; then
        status="fail-build"
        error_msg="bun build produced no output file"
        error_stage="bundle"

        end_time=$(now_ms)
        duration_ms=$(( end_time - start_time ))
        write_result "${idx}" "${pkg_name}" "${pkg_version}" "${status}" "${bundle_ok}" "${componentize_ok}" "${error_msg}" "${error_stage}" "${native_binding}" "${duration_ms}"
        return
    fi

    bundle_ok=true

    # Step 5: Componentize with jco
    log "  [${pkg_name}] Componentizing..."
    local componentize_output
    local wasm_file="${pkg_dir}/handler.wasm"
    if ! componentize_output=$("${JCO_BIN}" componentize \
        "${bundle_file}" \
        --wit "${WIT_DIR}" \
        --world-name handler \
        --enable http \
        --enable fetch-event \
        -o "${wasm_file}" 2>&1); then
        status="fail-build"
        error_msg="${componentize_output}"
        error_stage="componentize"

        end_time=$(now_ms)
        duration_ms=$(( end_time - start_time ))
        write_result "${idx}" "${pkg_name}" "${pkg_version}" "${status}" "${bundle_ok}" "${componentize_ok}" "${error_msg}" "${error_stage}" "${native_binding}" "${duration_ms}"
        return
    fi

    if [ ! -f "${wasm_file}" ]; then
        status="fail-build"
        error_msg="jco componentize produced no output file"
        error_stage="componentize"

        end_time=$(now_ms)
        duration_ms=$(( end_time - start_time ))
        write_result "${idx}" "${pkg_name}" "${pkg_version}" "${status}" "${bundle_ok}" "${componentize_ok}" "${error_msg}" "${error_stage}" "${native_binding}" "${duration_ms}"
        return
    fi

    componentize_ok=true

    # Step 6: Validate with wasm-tools (optional, non-fatal)
    if command -v wasm-tools &>/dev/null; then
        if ! wasm-tools component wit "${wasm_file}" > /dev/null 2>&1; then
            log "  [${pkg_name}] Warning: wasm-tools validation failed (component may still work)"
        fi
    fi

    log "  [${pkg_name}] PASS"

    end_time=$(now_ms)
    duration_ms=$(( end_time - start_time ))
    write_result "${idx}" "${pkg_name}" "${pkg_version}" "${status}" "${bundle_ok}" "${componentize_ok}" "${error_msg}" "${error_stage}" "${native_binding}" "${duration_ms}"
}

# ── Write a result to a numbered file (avoids subshell variable issues) ─

write_result() {
    local idx="$1"
    local name="$2"
    local version="$3"
    local status="$4"
    local bundle_ok="$5"
    local componentize_ok="$6"
    local error_msg="$7"
    local error_stage="$8"
    local native_binding="$9"
    local duration_ms="${10}"

    # Escape the error message for JSON
    local escaped_error
    escaped_error=$(printf '%s' "${error_msg}" | head -20 | tr '\n' ' ' | sed 's/\\/\\\\/g; s/"/\\"/g; s/\t/ /g' | cut -c1-500)

    local result_file="${RESULTS_DIR}/${idx}.json"

    {
        printf '{\n'
        printf '  "name": "%s",\n' "${name}"
        printf '  "version": "%s",\n' "${version}"
        printf '  "status": "%s",\n' "${status}"
        printf '  "bundle_ok": %s,\n' "${bundle_ok}"
        printf '  "componentize_ok": %s' "${componentize_ok}"

        if [ -n "${error_msg}" ]; then
            printf ',\n  "error": "%s"' "${escaped_error}"
            printf ',\n  "error_stage": "%s"' "${error_stage}"
        fi

        if [ "${native_binding}" = "true" ]; then
            printf ',\n  "native_binding": true'
        fi

        printf ',\n  "duration_ms": %s\n' "${duration_ms}"
        printf '}\n'
    } > "${result_file}"
}

# ── Assemble results JSON from individual result files ─────────────────

assemble_results() {
    local generated_at
    generated_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

    local total=0
    local pass=0
    local fail=0
    local entries=""

    for result_file in "${RESULTS_DIR}"/*.json; do
        [ -f "${result_file}" ] || continue
        total=$((total + 1))

        local entry_status
        entry_status=$(bun -e "console.log(require('${result_file}').status)")

        if [ "${entry_status}" = "pass" ]; then
            pass=$((pass + 1))
        else
            fail=$((fail + 1))
        fi

        local content
        content=$(cat "${result_file}")

        if [ -n "${entries}" ]; then
            entries="${entries},
    ${content}"
        else
            entries="    ${content}"
        fi
    done

    cat > "${RESULTS_JSON}" <<EOF
{
  "generated_at": "${generated_at}",
  "pipeline_version": "bun-build+jco-componentize",
  "total": ${total},
  "pass": ${pass},
  "fail": ${fail},
  "results": [
${entries}
  ]
}
EOF

    echo "${total}|${pass}|${fail}"
}

# ── Main ───────────────────────────────────────────────────────────────

log "WarpGrid Bun Compatibility Test Harness"
log "Pipeline: bun build → jco componentize → wasm-tools validate"
log ""

# Write package list to temp file (avoids subshell/mapfile portability issues)
PKG_LIST_FILE="${WORK_DIR}/_pkg_list.txt"
bun -e "
  const p = require('${PACKAGES_JSON}');
  for (const pkg of p.packages) {
    console.log(pkg.name + '|' + pkg.version);
  }
" > "${PKG_LIST_FILE}"

PACKAGE_COUNT=$(wc -l < "${PKG_LIST_FILE}" | tr -d ' ')
log "Testing ${PACKAGE_COUNT} packages..."
log ""

# Parse --only filter set
ONLY_SET=""
if [ -n "${ONLY_PACKAGES}" ]; then
    ONLY_SET=",${ONLY_PACKAGES},"
fi

# Iterate packages (read from file, not pipe — keeps variables in main shell)
IDX=0
while IFS='|' read -r pkg_name pkg_version; do
    # Filter if --only specified
    if [ -n "${ONLY_SET}" ]; then
        case ",${ONLY_PACKAGES}," in
            *",${pkg_name},"*) ;;
            *) continue ;;
        esac
    fi

    IDX=$((IDX + 1))
    IDX_PADDED=$(printf "%03d" "${IDX}")

    log "Testing [${IDX}/${PACKAGE_COUNT}]: ${pkg_name}@${pkg_version}"
    test_package "${pkg_name}" "${pkg_version}" "${IDX_PADDED}"
    log ""
done < "${PKG_LIST_FILE}"

# Assemble final results
SUMMARY=$(assemble_results)
IFS='|' read -r TOTAL PASS FAIL <<< "${SUMMARY}"

# Print summary
log "─────────────────────────────────────────────"
log "Results: ${PASS}/${TOTAL} passed, ${FAIL} failed"
log "Output:  ${RESULTS_JSON}"
log "─────────────────────────────────────────────"

if [ "${JSON_ONLY}" = true ]; then
    cat "${RESULTS_JSON}"
fi

# Exit 0 always — failures are recorded, not fatal
# CI regression detection is handled by the nightly workflow
exit 0
