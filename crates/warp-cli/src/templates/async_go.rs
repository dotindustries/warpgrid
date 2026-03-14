use super::TemplateFile;

pub fn files() -> Vec<TemplateFile> {
    vec![
        TemplateFile {
            path: "go.mod",
            content: GO_MOD,
        },
        TemplateFile {
            path: "main.go",
            content: MAIN_GO,
        },
        TemplateFile {
            path: "main_test.go",
            content: MAIN_TEST_GO,
        },
        TemplateFile {
            path: "warp.toml",
            content: WARP_TOML,
        },
        TemplateFile {
            path: "README.md",
            content: README,
        },
    ]
}

const GO_MOD: &str = r#"module my-async-handler

go 1.22.0

require github.com/anthropics/warpgrid/packages/warpgrid-go v0.0.0
"#;

const WARP_TOML: &str = r#"[package]
name = "my-async-handler"
version = "0.1.0"

[build]
lang = "go"
entry = "main.go"
"#;

const README: &str = r#"# Async Go Handler

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
"#;

const MAIN_GO: &str = r#"// Package main implements a WarpGrid async handler in Go.
//
// Routes:
//   - /health — returns {"status":"ok"}
//   - /       — echoes request method and URI as JSON
//
// Build: tinygo build -target=wasi -buildmode=c-shared -o my-async-handler.wasm .
// Reactor mode (-buildmode=c-shared) is required so that //go:wasmexport
// functions can be called after _initialize.
package main

import (
	"encoding/json"
	"fmt"
	"net/http"

	wghttp "github.com/anthropics/warpgrid/packages/warpgrid-go/http"
)

func init() {
	wghttp.HandleFunc("/health", handleHealth)
	wghttp.HandleFunc("/", handleEcho)
	wghttp.ListenAndServe(":0", nil)
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	fmt.Fprint(w, `{"status":"ok"}`)
}

func handleEcho(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	resp := map[string]interface{}{
		"method": r.Method,
		"uri":    r.URL.String(),
	}
	json.NewEncoder(w).Encode(resp)
}

func main() {
	// main is intentionally empty. Handler registration happens in init()
	// so that the module works correctly in reactor mode (-buildmode=c-shared)
	// where _initialize runs init() functions but not main().
}
"#;

const MAIN_TEST_GO: &str = r#"package main

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestHealthHandler(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/health", nil)
	w := httptest.NewRecorder()
	handleHealth(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("expected status 200, got %d", w.Code)
	}
	if ct := w.Header().Get("Content-Type"); ct != "application/json" {
		t.Errorf("expected application/json, got %s", ct)
	}

	var body map[string]string
	if err := json.Unmarshal(w.Body.Bytes(), &body); err != nil {
		t.Fatalf("failed to parse response: %v", err)
	}
	if body["status"] != "ok" {
		t.Errorf("expected status ok, got %s", body["status"])
	}
}

func TestEchoHandler(t *testing.T) {
	req := httptest.NewRequest(http.MethodPost, "/test-path", nil)
	w := httptest.NewRecorder()
	handleEcho(w, req)

	if w.Code != http.StatusOK {
		t.Errorf("expected status 200, got %d", w.Code)
	}

	var body map[string]interface{}
	if err := json.Unmarshal(w.Body.Bytes(), &body); err != nil {
		t.Fatalf("failed to parse response: %v", err)
	}
	if body["method"] != "POST" {
		t.Errorf("expected method POST, got %s", body["method"])
	}
	if body["uri"] != "/test-path" {
		t.Errorf("expected uri /test-path, got %s", body["uri"])
	}
}
"#;
