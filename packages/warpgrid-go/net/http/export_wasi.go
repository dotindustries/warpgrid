// WASI-specific export bridge for WarpGrid HTTP handler invocation.
//
// This file is only compiled when targeting WASI (wasip2).
// The host calls warpgrid_http_handle_request with a pointer to the
// serialized WIT http-request in linear memory. The guest processes
// it through the registered handler and returns a pointer and length
// to the serialized WIT http-response.

//go:build wasip2

package http

import "unsafe"

// lastResponse holds the most recent response to keep it alive in
// linear memory until the host reads it (prevents GC collection).
var lastResponse []byte

// warpgridHttpHandleRequest is the WASI export entry point.
// The host serializes an http-request into the guest's linear memory,
// calls this function, then reads the response from the returned pointer.
//
//go:wasmexport warpgrid_http_handle_request
func warpgridHttpHandleRequest(reqPtr *byte, reqLen uint32) (respPtr *byte, respLen uint32) {
	reqBytes := unsafe.Slice(reqPtr, reqLen)
	lastResponse = HandleRequest(reqBytes)
	if len(lastResponse) == 0 {
		return nil, 0
	}
	return &lastResponse[0], uint32(len(lastResponse))
}
