# Deploy a Go App

This guide walks through deploying a Go HTTP microservice to WarpGrid.

## Prerequisites

- The `warp` CLI
- [Go](https://go.dev) 1.21+ installed locally
- [TinyGo](https://tinygo.org) installed (WarpGrid uses TinyGo to compile Go
  to `wasm32-wasip2`)
- A WarpGrid Cloud account (`warp login`)

## Create the Project

Scaffold from the async-go template:

```bash
warp init --template async-go my-go-api
cd my-go-api
```

Generated structure:

```
my-go-api/
  warp.toml
  go.mod
  main.go
```

## Understand the Manifest

```toml
[package]
name = "my-go-api"
version = "0.1.0"
description = "Minimal Go HTTP microservice example for WarpGrid"

[build]
lang = "go"
entry = "main.go"
target = "wasip2"

[runtime]
trigger = "http"
```

The `lang = "go"` field tells `warp pack` to compile via TinyGo targeting
`wasm32-wasip2`.

## Write the Handler

Go handlers use the standard `net/http` package. Here is a microservice with
CRUD routes (from `examples/go-microservice`):

```go
package main

import (
    "encoding/json"
    "log"
    "net/http"
    "os"
    "time"
)

type Item struct {
    ID   int    `json:"id"`
    Name string `json:"name"`
}

var (
    items  []Item
    nextID int
)

func main() {
    port := os.Getenv("PORT")
    if port == "" {
        port = "8080"
    }

    http.HandleFunc("/", handleIndex)
    http.HandleFunc("/health", handleHealth)
    http.HandleFunc("/items", handleItems)

    log.Printf("listening on :%s\n", port)
    log.Fatal(http.ListenAndServe(":"+port, nil))
}

func writeJSON(w http.ResponseWriter, status int, data any) {
    w.Header().Set("Content-Type", "application/json")
    w.WriteHeader(status)
    json.NewEncoder(w).Encode(data)
}

func handleIndex(w http.ResponseWriter, r *http.Request) {
    writeJSON(w, http.StatusOK, map[string]string{
        "message":   "Hello from WarpGrid!",
        "runtime":   "go",
        "timestamp": time.Now().UTC().Format(time.RFC3339),
    })
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
    writeJSON(w, http.StatusOK, map[string]string{"status": "ok"})
}

func handleItems(w http.ResponseWriter, r *http.Request) {
    switch r.Method {
    case http.MethodGet:
        result := items
        if result == nil {
            result = []Item{}
        }
        writeJSON(w, http.StatusOK, result)
    case http.MethodPost:
        var input struct {
            Name string `json:"name"`
        }
        if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
            writeJSON(w, http.StatusBadRequest,
                map[string]string{"error": "Invalid JSON"})
            return
        }
        nextID++
        item := Item{ID: nextID, Name: input.Name}
        items = append(items, item)
        writeJSON(w, http.StatusCreated, item)
    }
}
```

The code is plain Go. There is no WarpGrid-specific import or annotation.

## Test Locally

```bash
go run main.go
```

```bash
curl http://localhost:8080/
curl -X POST http://localhost:8080/items -d '{"name":"widget"}'
curl http://localhost:8080/items
```

## Check Compatibility

Before deploying, verify your dependencies compile to Wasm:

```bash
warp convert analyze --path . --lang go
```

The analyzer scans `go.mod`, checks each dependency against the compatibility
database, and reports any blockers. Pure-Go libraries generally work. Libraries
that use cgo or raw syscalls will be flagged.

## Deploy

```bash
warp deploy --region iad
```

Output:

```
Compiling project...
  Compiled: target/my-go-api.wasm (3.1 MB, sha256: b2c3d4e5f6a7)
Deploying 'my-go-api' to iad (3254780 bytes)...
Deployed successfully!
  Name:      my-go-api
  URL:       https://my-go-api.you.edge.warpgrid.dev
  Wasm hash: b2c3d4e5f6a7
```

## Enable Shims for Database Access

If your Go service connects to PostgreSQL, enable the database proxy shim:

```toml
[shims]
database_proxy = true
dns = true
```

This routes database connections through WarpGrid's transparent proxy, which
handles TLS termination and connection pooling on the host side.

## Full Example

See [`examples/go-microservice/`](https://github.com/dotindustries/warpgrid/tree/main/examples/go-microservice)
in the repository.
