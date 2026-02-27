#!/usr/bin/env python3
"""
mock-registry — Lightweight HTTP mock for WarpGrid service discovery.

Endpoints:
    GET /health            → { "status": "ok" }
    GET /services          → Full service registry
    GET /resolve/<name>    → Resolve a single service name

Runs on port 8888 by default (configurable via PORT env var).
"""

import json
import os
from http.server import HTTPServer, BaseHTTPRequestHandler

PORT = int(os.environ.get("PORT", "8888"))

SERVICES = [
    {
        "name": "db.test.warp.local",
        "addresses": ["172.20.0.10"],
        "port": 5432,
        "protocol": "postgres",
    },
    {
        "name": "cache.test.warp.local",
        "addresses": ["172.20.0.11"],
        "port": 6379,
        "protocol": "redis",
    },
    {
        "name": "user-svc.test.warp.local",
        "addresses": ["172.20.0.20"],
        "port": 8080,
        "protocol": "http",
    },
    {
        "name": "notification-svc.test.warp.local",
        "addresses": ["172.20.0.21"],
        "port": 8080,
        "protocol": "http",
    },
    {
        "name": "analytics-svc.test.warp.local",
        "addresses": ["172.20.0.22"],
        "port": 8080,
        "protocol": "http",
    },
]


class RegistryHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        path = self.path.rstrip("/")

        if path == "/health":
            self._json_response({"status": "ok", "service": "mock-registry"})
        elif path == "/services":
            self._json_response({"services": SERVICES})
        elif path.startswith("/resolve/"):
            name = path[len("/resolve/"):]
            self._resolve(name)
        else:
            self._json_response({"error": "not found", "path": self.path}, 404)

    def _resolve(self, name):
        for svc in SERVICES:
            if svc["name"] == name or svc["name"].startswith(name):
                self._json_response({
                    "name": svc["name"],
                    "addresses": svc["addresses"],
                    "port": svc["port"],
                })
                return
        self._json_response({"error": "service not found", "name": name}, 404)

    def _json_response(self, data, status=200):
        body = json.dumps(data).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        # Quiet logging — only errors
        pass


if __name__ == "__main__":
    server = HTTPServer(("0.0.0.0", PORT), RegistryHandler)
    print(f"mock-registry listening on port {PORT}")
    server.serve_forever()
