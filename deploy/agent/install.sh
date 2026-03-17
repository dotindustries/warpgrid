#!/usr/bin/env bash
#
# WarpGrid Agent — BYOC (Bring-Your-Own-Compute) installer.
#
# Installs warpd agent on a KVM-capable Linux host and registers it
# with the WarpGrid cloud control plane. The agent creates and manages
# sprite VMs on behalf of the customer's namespace.
#
# Usage:
#   curl -fsSL https://get.warpgrid.dev/agent | bash -s -- \
#     --token wg_agent_... \
#     --control-plane cloud.warpgrid.dev:50051 \
#     --region iad
#
# Requirements:
#   - Linux x86_64 with KVM support (/dev/kvm)
#   - Root or sudo access
#   - curl, tar
#
# What it does:
#   1. Validates KVM support
#   2. Downloads warpd binary + cloud-hypervisor + kernel + golden image
#   3. Installs to /opt/warpgrid/
#   4. Creates a systemd service (warpgrid-agent.service)
#   5. Starts the agent (joins the cloud control plane)

set -euo pipefail

# ── Defaults ──────────────────────────────────────────────────────

WARPGRID_VERSION="${WARPGRID_VERSION:-latest}"
WARPGRID_CONTROL_PLANE="${WARPGRID_CONTROL_PLANE:-cloud.warpgrid.dev:50051}"
WARPGRID_REGION="${WARPGRID_REGION:-iad}"
WARPGRID_DATA_DIR="${WARPGRID_DATA_DIR:-/var/lib/warpgrid}"
WARPGRID_INSTALL_DIR="${WARPGRID_INSTALL_DIR:-/opt/warpgrid}"
WARPGRID_AGENT_TOKEN=""
WARPGRID_ADVERTISE_ADDRESS=""
WARPGRID_CAPACITY_MEMORY=""
WARPGRID_CAPACITY_CPU=""

# ── Parse args ────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
    case "$1" in
        --token)           WARPGRID_AGENT_TOKEN="$2"; shift 2 ;;
        --control-plane)   WARPGRID_CONTROL_PLANE="$2"; shift 2 ;;
        --region)          WARPGRID_REGION="$2"; shift 2 ;;
        --address)         WARPGRID_ADVERTISE_ADDRESS="$2"; shift 2 ;;
        --data-dir)        WARPGRID_DATA_DIR="$2"; shift 2 ;;
        --version)         WARPGRID_VERSION="$2"; shift 2 ;;
        --memory)          WARPGRID_CAPACITY_MEMORY="$2"; shift 2 ;;
        --cpu)             WARPGRID_CAPACITY_CPU="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: install.sh --token <wg_agent_...> [--control-plane host:port] [--region region]"
            echo ""
            echo "Options:"
            echo "  --token           Agent auth token (required, from console or API)"
            echo "  --control-plane   Control plane gRPC endpoint (default: cloud.warpgrid.dev:50051)"
            echo "  --region          Region identifier (default: iad)"
            echo "  --address         Advertise address (default: auto-detect)"
            echo "  --data-dir        Data directory (default: /var/lib/warpgrid)"
            echo "  --version         WarpGrid version (default: latest)"
            echo "  --memory          Memory capacity in bytes (default: auto-detect)"
            echo "  --cpu             CPU weight capacity (default: auto-detect)"
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# ── Validation ────────────────────────────────────────────────────

if [[ -z "$WARPGRID_AGENT_TOKEN" ]]; then
    echo "ERROR: --token is required. Get one from https://cloud.warpgrid.dev/console or via API:"
    echo "  curl -X POST https://cloud.warpgrid.dev/api/v1/cloud/agent-tokens \\"
    echo "    -H 'Authorization: Bearer wg_live_...' \\"
    echo '    -d '"'"'{"name": "my-node"}'"'"
    exit 1
fi

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "ERROR: WarpGrid agent requires Linux. Detected: $(uname -s)"
    exit 1
fi

if [[ "$(uname -m)" != "x86_64" ]]; then
    echo "ERROR: WarpGrid agent requires x86_64. Detected: $(uname -m)"
    exit 1
fi

if [[ ! -e /dev/kvm ]]; then
    echo "ERROR: KVM is not available. Ensure the host has hardware virtualization enabled"
    echo "  and the kvm module is loaded (modprobe kvm_intel or kvm_amd)."
    exit 1
fi

if [[ ! -w /dev/kvm ]]; then
    echo "ERROR: /dev/kvm is not writable. Run as root or add your user to the kvm group."
    exit 1
fi

# ── Auto-detect advertise address ─────────────────────────────────

if [[ -z "$WARPGRID_ADVERTISE_ADDRESS" ]]; then
    WARPGRID_ADVERTISE_ADDRESS=$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{print $7; exit}' || hostname -I | awk '{print $1}')
    echo "Auto-detected advertise address: $WARPGRID_ADVERTISE_ADDRESS"
fi

# ── Install ───────────────────────────────────────────────────────

echo "Installing WarpGrid agent..."
echo "  Control plane: $WARPGRID_CONTROL_PLANE"
echo "  Region:        $WARPGRID_REGION"
echo "  Data dir:      $WARPGRID_DATA_DIR"
echo "  Install dir:   $WARPGRID_INSTALL_DIR"

mkdir -p "$WARPGRID_INSTALL_DIR/bin"
mkdir -p "$WARPGRID_DATA_DIR"

# Download warpd binary.
DOWNLOAD_BASE="https://releases.warpgrid.dev/${WARPGRID_VERSION}"
echo "Downloading warpd..."
curl -fsSL "${DOWNLOAD_BASE}/warpd-linux-x86_64.tar.gz" | tar -xz -C "$WARPGRID_INSTALL_DIR/bin"
chmod +x "$WARPGRID_INSTALL_DIR/bin/warpd"

# Download cloud-hypervisor (VMM).
echo "Downloading cloud-hypervisor..."
curl -fsSL "${DOWNLOAD_BASE}/cloud-hypervisor-linux-x86_64" -o "$WARPGRID_INSTALL_DIR/bin/cloud-hypervisor"
chmod +x "$WARPGRID_INSTALL_DIR/bin/cloud-hypervisor"

# Download kernel + golden image for sprite VMs.
echo "Downloading sprite kernel and base image..."
curl -fsSL "${DOWNLOAD_BASE}/vmlinux-sprite" -o "$WARPGRID_DATA_DIR/vmlinux-sprite"
curl -fsSL "${DOWNLOAD_BASE}/sprite-base.raw" -o "$WARPGRID_DATA_DIR/sprite-base.raw"

echo "Binaries installed to $WARPGRID_INSTALL_DIR/bin/"

# ── Build systemd unit ────────────────────────────────────────────

EXTRA_ARGS=""
if [[ -n "$WARPGRID_CAPACITY_MEMORY" ]]; then
    EXTRA_ARGS="$EXTRA_ARGS --capacity-memory-bytes $WARPGRID_CAPACITY_MEMORY"
fi
if [[ -n "$WARPGRID_CAPACITY_CPU" ]]; then
    EXTRA_ARGS="$EXTRA_ARGS --capacity-cpu-weight $WARPGRID_CAPACITY_CPU"
fi

cat > /etc/systemd/system/warpgrid-agent.service <<EOF
[Unit]
Description=WarpGrid Agent (BYOC)
Documentation=https://docs.warpgrid.dev/byoc
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$WARPGRID_INSTALL_DIR/bin/warpd agent \\
    --control-plane $WARPGRID_CONTROL_PLANE \\
    --auth-token $WARPGRID_AGENT_TOKEN \\
    --address $WARPGRID_ADVERTISE_ADDRESS \\
    --region $WARPGRID_REGION \\
    --data-dir $WARPGRID_DATA_DIR $EXTRA_ARGS
Restart=always
RestartSec=5
Environment=RUST_LOG=info,warpd=debug

# Security hardening
NoNewPrivileges=no
ProtectSystem=strict
ReadWritePaths=$WARPGRID_DATA_DIR
PrivateTmp=true

[Install]
WantedBy=multi-user.target
EOF

# ── Start service ─────────────────────────────────────────────────

echo "Starting warpgrid-agent service..."
systemctl daemon-reload
systemctl enable warpgrid-agent
systemctl start warpgrid-agent

echo ""
echo "WarpGrid agent installed and running!"
echo ""
echo "  Status:  systemctl status warpgrid-agent"
echo "  Logs:    journalctl -u warpgrid-agent -f"
echo "  Stop:    systemctl stop warpgrid-agent"
echo "  Remove:  systemctl disable warpgrid-agent && rm /etc/systemd/system/warpgrid-agent.service"
