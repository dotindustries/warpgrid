# WarpGrid daemon — multi-stage build for minimal image.
#
# Produces a ~30MB image containing only the warpd binary.
# Supports both cloud (control plane) and agent (edge node) modes.
#
# Build:
#   docker build -t warpgrid -f infra/Dockerfile .
#
# Run:
#   docker run -p 8443:8443 warpgrid cloud --api-port 8443
#   docker run -p 8443:8443 warpgrid agent --control-plane <url>

FROM rust:1.94-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev cmake protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# Build only the warpd binary in release mode.
RUN cargo build --release --package warpd \
    && strip target/release/warpd

# ── Runtime image ────────────────────────────────────────────────
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/warpd /usr/local/bin/warpd

RUN mkdir -p /var/lib/warpgrid

EXPOSE 8443
EXPOSE 50051

ENTRYPOINT ["warpd"]
CMD ["standalone", "--port", "8443"]
