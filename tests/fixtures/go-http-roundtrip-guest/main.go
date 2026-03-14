// Package main implements a Go HTTP handler fixture for integration testing
// the WarpGrid HTTP overlay round-trip (US-307, issue #45).
//
// Routes:
//   - /echo    — echoes request body with Content-Type preserved
//   - /status  — returns the status code from the query parameter ?code=NNN
//   - /headers — echoes request headers as JSON
//   - /method  — returns the HTTP method in the response body
//   - /stream  — reads body via io.Reader in chunks, reports byte count
//
// Build: tinygo build -target=wasip2 -o go-http-roundtrip.wasm .
// The WASI export bridge in wghttp.export_wasi.go provides the
// warpgrid-handle-request core module export.
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strconv"

	wghttp "github.com/anthropics/warpgrid/packages/warpgrid-go/http"
)

func main() {
	wghttp.HandleFunc("/echo", echoHandler)
	wghttp.HandleFunc("/status", statusHandler)
	wghttp.HandleFunc("/headers", headersHandler)
	wghttp.HandleFunc("/method", methodHandler)
	wghttp.HandleFunc("/stream", streamHandler)

	// ListenAndServe registers the handler with WarpGrid — it does not
	// open a socket. The addr is informational only.
	wghttp.ListenAndServe(":0", nil)
}

// echoHandler echoes the request body back with the same Content-Type.
func echoHandler(w http.ResponseWriter, r *http.Request) {
	ct := r.Header.Get("Content-Type")
	if ct != "" {
		w.Header().Set("Content-Type", ct)
	}
	body, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "read body: "+err.Error(), http.StatusInternalServerError)
		return
	}
	w.Write(body)
}

// statusHandler returns the HTTP status code specified in the ?code= query param.
func statusHandler(w http.ResponseWriter, r *http.Request) {
	codeStr := r.URL.Query().Get("code")
	if codeStr == "" {
		http.Error(w, "missing ?code= parameter", http.StatusBadRequest)
		return
	}
	code, err := strconv.Atoi(codeStr)
	if err != nil {
		http.Error(w, "invalid code: "+err.Error(), http.StatusBadRequest)
		return
	}
	w.Header().Set("Content-Type", "text/plain")
	w.WriteHeader(code)
	fmt.Fprintf(w, "status: %d", code)
}

// headersHandler echoes back request headers as a JSON object.
func headersHandler(w http.ResponseWriter, r *http.Request) {
	headerMap := make(map[string][]string)
	for name, values := range r.Header {
		headerMap[name] = values
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(headerMap)
}

// methodHandler returns the HTTP method in the response body.
func methodHandler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("Content-Type", "text/plain")
	w.Write([]byte(r.Method))
}

// streamHandler reads the body in 1024-byte chunks and reports the total byte count.
func streamHandler(w http.ResponseWriter, r *http.Request) {
	buf := make([]byte, 1024)
	total := 0
	for {
		n, err := r.Body.Read(buf)
		total += n
		if err == io.EOF {
			break
		}
		if err != nil {
			http.Error(w, "stream read: "+err.Error(), http.StatusInternalServerError)
			return
		}
	}
	w.Header().Set("Content-Type", "text/plain")
	fmt.Fprintf(w, "bytes_read: %d", total)
}
