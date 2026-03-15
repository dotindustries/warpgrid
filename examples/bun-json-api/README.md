# Bun JSON API

A minimal Bun REST API deployed on WarpGrid. Demonstrates JSON routing, request echo, and environment variable filtering.

## Deploy

```bash
cd examples/bun-json-api
warp deploy
```

## Endpoints

- `GET /` — JSON greeting with runtime info and timestamp
- `GET /health` — Health check (`{"status": "ok"}`)
- `POST /echo` — Echoes back the request body as JSON
- `GET /env` — Returns safe environment variables (filtered by prefix)

## Local Development

```bash
warp dev
```
