// Package http provides an HTTP server overlay for WarpGrid WASI modules.
//
// On WASI targets, ListenAndServe does not open a TCP socket; instead it
// registers the handler with the WarpGrid trigger system. The runtime
// invokes the registered handler for each incoming HTTP request via the
// exported warpgrid_http_handle_request function.
//
// The API mirrors net/http so that switching import paths is a drop-in
// change for standard Go HTTP server code. TinyGo's -overlay flag can
// map "net/http" to this package at build time for transparent use.
//
// This package is part of the WarpGrid Go overlay (Domain 3, US-306).
package http

import (
	"bytes"
	"io"
	"net/url"
)

// HTTP method constants matching net/http.
const (
	MethodGet     = "GET"
	MethodHead    = "HEAD"
	MethodPost    = "POST"
	MethodPut     = "PUT"
	MethodPatch   = "PATCH"
	MethodDelete  = "DELETE"
	MethodConnect = "CONNECT"
	MethodOptions = "OPTIONS"
	MethodTrace   = "TRACE"
)

// HTTP status code constants matching net/http.
const (
	StatusOK                  = 200
	StatusCreated             = 201
	StatusNoContent           = 204
	StatusBadRequest          = 400
	StatusUnauthorized        = 401
	StatusForbidden           = 403
	StatusNotFound            = 404
	StatusMethodNotAllowed    = 405
	StatusInternalServerError = 500
	StatusBadGateway          = 502
	StatusServiceUnavailable  = 503
)

// Header represents HTTP headers as a map of header name to values.
// This matches the net/http.Header interface.
type Header map[string][]string

// Set sets the header entry associated with key to the single value.
func (h Header) Set(key, value string) {
	h[key] = []string{value}
}

// Get returns the first value associated with the given key.
// Returns empty string if the key is not present.
func (h Header) Get(key string) string {
	if values, ok := h[key]; ok && len(values) > 0 {
		return values[0]
	}
	return ""
}

// Add appends a value to the header associated with key.
func (h Header) Add(key, value string) {
	h[key] = append(h[key], value)
}

// Del removes the header entry associated with key.
func (h Header) Del(key string) {
	delete(h, key)
}

// Handler responds to an HTTP request.
type Handler interface {
	ServeHTTP(ResponseWriter, *Request)
}

// HandlerFunc adapts ordinary functions to the Handler interface.
type HandlerFunc func(ResponseWriter, *Request)

// ServeHTTP calls f(w, r).
func (f HandlerFunc) ServeHTTP(w ResponseWriter, r *Request) {
	f(w, r)
}

// ResponseWriter interface for building HTTP responses.
// Matches the net/http.ResponseWriter interface.
type ResponseWriter interface {
	Header() Header
	Write([]byte) (int, error)
	WriteHeader(statusCode int)
}

// Request represents an incoming HTTP request.
type Request struct {
	Method string
	URL    *url.URL
	Header Header
	Body   io.ReadCloser
}

// NewRequest creates a Request from method, URI, and optional body.
// Used for testing and internal request construction.
func NewRequest(method, uri string, body []byte) *Request {
	u, _ := url.ParseRequestURI(uri)
	if u == nil {
		u = &url.URL{Path: uri}
	}

	var bodyReader io.ReadCloser
	if body != nil {
		bodyReader = io.NopCloser(bytes.NewReader(body))
	} else {
		bodyReader = io.NopCloser(bytes.NewReader(nil))
	}

	return &Request{
		Method: method,
		URL:    u,
		Header: make(Header),
		Body:   bodyReader,
	}
}

// Error replies to the request with the specified error message and code.
// Matches the net/http.Error signature.
func Error(w ResponseWriter, error string, code int) {
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.WriteHeader(code)
	w.Write([]byte(error))
}
