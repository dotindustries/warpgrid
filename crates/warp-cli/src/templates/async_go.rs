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

go 1.22
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
- `go.mod` — Go module definition
- `warp.toml` — WarpGrid build configuration

## How It Works

The handler uses the `warpgrid-go/http` bridge to register HTTP route handlers.
`wghttp.HandleFunc()` registers handlers and `wghttp.ListenAndServe()` wires
them into the WarpGrid runtime. Standard `net/http` handler signatures are used.
"#;

const MAIN_GO: &str = r#"package main

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

func main() {}
"#;
