#!/bin/sh
#
# mock-registry/server.sh — Lightweight HTTP mock for WarpGrid service discovery.
#
# Endpoints:
#   GET /health    → { "status": "ok" }
#   GET /services  → WarpGrid service discovery responses
#   GET /resolve/:name → Resolve a single service name
#
# Uses busybox httpd with CGI for zero-dependency Alpine container.

set -eu

PORT="${PORT:-8888}"
DOC_ROOT="/srv/www"

mkdir -p "${DOC_ROOT}/cgi-bin"

# Create the CGI handler
cat > "${DOC_ROOT}/cgi-bin/api.sh" << 'HANDLER'
#!/bin/sh
# CGI script for mock service discovery

# Parse request
REQUEST_PATH="${PATH_INFO:-/}"

# Service registry data
SERVICES='{
  "services": [
    {
      "name": "db.test.warp.local",
      "addresses": ["172.20.0.10"],
      "port": 5432,
      "protocol": "postgres"
    },
    {
      "name": "cache.test.warp.local",
      "addresses": ["172.20.0.11"],
      "port": 6379,
      "protocol": "redis"
    },
    {
      "name": "user-svc.test.warp.local",
      "addresses": ["172.20.0.20"],
      "port": 8080,
      "protocol": "http"
    },
    {
      "name": "notification-svc.test.warp.local",
      "addresses": ["172.20.0.21"],
      "port": 8080,
      "protocol": "http"
    },
    {
      "name": "analytics-svc.test.warp.local",
      "addresses": ["172.20.0.22"],
      "port": 8080,
      "protocol": "http"
    }
  ]
}'

case "$REQUEST_PATH" in
  /health)
    printf "Content-Type: application/json\r\n\r\n"
    printf '{"status":"ok","service":"mock-registry"}\n'
    ;;
  /services)
    printf "Content-Type: application/json\r\n\r\n"
    printf '%s\n' "$SERVICES"
    ;;
  /resolve/*)
    SERVICE_NAME="${REQUEST_PATH#/resolve/}"
    printf "Content-Type: application/json\r\n\r\n"
    # Simple lookup by name substring
    case "$SERVICE_NAME" in
      db.*|*postgres*)
        printf '{"name":"db.test.warp.local","addresses":["172.20.0.10"],"port":5432}\n'
        ;;
      cache.*|*redis*)
        printf '{"name":"cache.test.warp.local","addresses":["172.20.0.11"],"port":6379}\n'
        ;;
      user-svc.*)
        printf '{"name":"user-svc.test.warp.local","addresses":["172.20.0.20"],"port":8080}\n'
        ;;
      notification-svc.*)
        printf '{"name":"notification-svc.test.warp.local","addresses":["172.20.0.21"],"port":8080}\n'
        ;;
      analytics-svc.*)
        printf '{"name":"analytics-svc.test.warp.local","addresses":["172.20.0.22"],"port":8080}\n'
        ;;
      *)
        printf '{"error":"service not found","name":"%s"}\n' "$SERVICE_NAME"
        ;;
    esac
    ;;
  *)
    printf "Content-Type: application/json\r\n\r\n"
    printf '{"error":"not found","path":"%s"}\n' "$REQUEST_PATH"
    ;;
esac
HANDLER

chmod +x "${DOC_ROOT}/cgi-bin/api.sh"

# Create httpd config
cat > /etc/httpd.conf << EOF
H:${DOC_ROOT}
A:*
/cgi-bin:${DOC_ROOT}/cgi-bin
EOF

echo "mock-registry listening on port ${PORT}"

# Run busybox httpd in foreground with CGI support
exec httpd -f -p "${PORT}" -h "${DOC_ROOT}" -c /etc/httpd.conf
