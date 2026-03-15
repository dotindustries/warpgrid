# Async Go Handler

A WarpGrid async handler written in Go.

## Prerequisites

- TinyGo (for WASI component compilation)
- `warp` CLI

## Getting Started

```bash
# Build the Wasm component
warp pack

# The output .wasm component will be in the project directory
```

## Project Structure

- `main.go` — Handler implementation
- `main_test.go` — Unit tests
- `go.mod` — Go module definition
- `warp.toml` — WarpGrid build configuration

## How It Works

The handler uses the `warpgrid-go/http` bridge to register HTTP route handlers.
`wghttp.HandleFunc()` registers handlers and `wghttp.ListenAndServe()` wires
them into the WarpGrid runtime. Standard `net/http` handler signatures are used.

Handler registration happens in `init()` (not `main()`) because the module runs
in reactor mode (`-buildmode=c-shared`), where `_initialize` runs `init()`
functions but not `main()`.

## Running Tests

```bash
go test ./...
```
