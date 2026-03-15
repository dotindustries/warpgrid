# Go Microservice

A minimal Go HTTP microservice deployed on WarpGrid. Demonstrates JSON routing, in-memory CRUD, and input validation.

## Deploy

```bash
cd examples/go-microservice
warp deploy
```

## Endpoints

- `GET /` — JSON greeting with timestamp
- `GET /health` — Health check (`{"status": "ok"}`)
- `POST /items` — Create an item (JSON body with `name` field), returns 201
- `GET /items` — List all items

## Local Development

```bash
warp dev
```
