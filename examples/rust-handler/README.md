# Rust Handler

A minimal Rust `#![no_std]` WASI component handler deployed on WarpGrid. Demonstrates WIT-based HTTP handling with JSON responses and string transformation.

## Deploy

```bash
cd examples/rust-handler
warp deploy
```

## Endpoints

- `GET /` — JSON greeting (`{"message": "Hello from WarpGrid!", "runtime": "rust"}`)
- `GET /health` — Health check (`{"status": "ok"}`)
- `POST /uppercase` — Converts request body to uppercase, returns as JSON

## Local Development

```bash
warp dev
```
