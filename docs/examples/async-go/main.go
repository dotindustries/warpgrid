// Package main implements a WarpGrid async handler in Go.
//
// Routes:
//   - /health — returns {"status":"ok"}
//   - /       — echoes request method and URI as JSON
//
// Build: tinygo build -target=wasi -buildmode=c-shared -o async-go-template.wasm .
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
