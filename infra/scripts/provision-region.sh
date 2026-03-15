#!/usr/bin/env bash
#
# provision-region.sh — Add a new edge region to the WarpGrid cloud platform.
#
# Usage:
#   ./infra/scripts/provision-region.sh <region>
#   ./infra/scripts/provision-region.sh ams
#   ./infra/scripts/provision-region.sh --list
#
# Prerequisites:
#   - fly CLI installed and authenticated
#   - FLY_API_TOKEN environment variable set

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

EDGE_APP="warpgrid-edge"
EDGE_IMAGE="registry.fly.io/${EDGE_APP}:latest"

usage() {
    cat <<'EOF'
Usage: provision-region.sh <region> | --list | --help

Provision a new WarpGrid edge agent in a Fly.io region.

Arguments:
  <region>    Fly.io region code (e.g., iad, ams, sin, gru, syd)
  --list      List all currently provisioned edge machines
  --help      Show this help

Environment:
  FLY_API_TOKEN   Required. Fly.io API token.

Examples:
  provision-region.sh iad       # US East (Ashburn)
  provision-region.sh ams       # Europe (Amsterdam)
  provision-region.sh sin       # Asia (Singapore)
  provision-region.sh --list    # Show existing machines
EOF
}

list_machines() {
    echo "Edge machines in ${EDGE_APP}:"
    fly machines list --app "${EDGE_APP}" 2>/dev/null || echo "  (none or app not found)"
}

provision() {
    local region="$1"

    if [ -z "${FLY_API_TOKEN:-}" ]; then
        echo "Error: FLY_API_TOKEN not set" >&2
        exit 1
    fi

    echo "Provisioning edge machine in region: ${region}"

    # Check if machine already exists in this region.
    local existing
    existing=$(fly machines list --app "${EDGE_APP}" --json 2>/dev/null | \
        python3 -c "import sys,json; ms=json.load(sys.stdin); print(sum(1 for m in ms if m.get('region')=='${region}'))" 2>/dev/null || echo "0")

    if [ "${existing}" -gt 0 ]; then
        echo "Machine already exists in ${region}. Skipping."
        exit 0
    fi

    fly machine run "${EDGE_IMAGE}" \
        --app "${EDGE_APP}" \
        --region "${region}" \
        --name "warpgrid-edge-${region}" \
        --vm-size shared-cpu-2x \
        --vm-memory 1024 \
        --port 443:8443/tcp:tls:http \
        --port 80:8443/tcp:http \
        --env RUST_LOG=info,warpd=debug \
        --env WARPGRID_REGION="${region}"

    echo "Edge machine provisioned in ${region}."
}

# ── Main ──────────────────────────────────────────────────────────

case "${1:-}" in
    --help|-h)
        usage
        exit 0
        ;;
    --list)
        list_machines
        exit 0
        ;;
    "")
        echo "Error: region required" >&2
        usage
        exit 1
        ;;
    *)
        provision "$1"
        ;;
esac
