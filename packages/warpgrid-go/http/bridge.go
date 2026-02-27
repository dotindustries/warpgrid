package wghttp

import (
	"fmt"
	"net/http"
)

// registeredHandler holds the handler set by ListenAndServe.
var registeredHandler http.Handler

// defaultServeMux is the WarpGrid-local default ServeMux. Separate from
// net/http.DefaultServeMux to avoid cross-contamination when the overlay
// is used alongside the standard library in tests.
var defaultServeMux = http.NewServeMux()

// ListenAndServe registers the handler with the WarpGrid trigger system.
//
// Unlike net/http.ListenAndServe, this does NOT open a socket. The addr
// parameter is informational only (the host manages port binding). If
// handler is nil, the WarpGrid default ServeMux is used.
//
// In WASI mode, this function returns nil immediately so that the module
// initialization completes and the host can call the exported handle-request
// function. In native Go mode (tests), it also returns nil immediately.
func ListenAndServe(addr string, handler http.Handler) error {
	if handler == nil {
		handler = defaultServeMux
	}
	registeredHandler = handler
	return nil
}

// HandleFunc registers the handler function for the given pattern on the
// WarpGrid default ServeMux.
func HandleFunc(pattern string, handler func(http.ResponseWriter, *http.Request)) {
	defaultServeMux.HandleFunc(pattern, handler)
}

// Handle registers the handler for the given pattern on the WarpGrid
// default ServeMux.
func Handle(pattern string, handler http.Handler) {
	defaultServeMux.Handle(pattern, handler)
}

// SetHandler directly sets the registered handler. Exposed for testing.
func SetHandler(handler http.Handler) {
	registeredHandler = handler
}

// ResetHandler clears the registered handler. Exposed for testing.
func ResetHandler() {
	registeredHandler = nil
}

// ResetDefaultServeMux replaces the default ServeMux with a fresh instance.
// Exposed for testing to avoid pattern registration leaking between tests.
func ResetDefaultServeMux() {
	defaultServeMux = http.NewServeMux()
}

// HandleWitRequest processes a WIT request through the registered handler
// and returns a WIT response.
//
// If no handler is registered, returns a 500 response. If the request
// conversion fails, returns a 400 response. Panics in the handler are
// recovered and converted to 500 responses.
func HandleWitRequest(req WitRequest) (resp WitResponse) {
	handler := registeredHandler
	if handler == nil {
		return WitResponse{
			Status:  500,
			Headers: []WitHeader{{Name: "Content-Type", Value: "text/plain"}},
			Body:    []byte("no handler registered"),
		}
	}

	httpReq, err := ConvertRequest(req)
	if err != nil {
		return WitResponse{
			Status:  400,
			Headers: []WitHeader{{Name: "Content-Type", Value: "text/plain"}},
			Body:    []byte("invalid request: " + err.Error()),
		}
	}

	rc := NewResponseCapture()

	// Recover from handler panics to avoid crashing the Wasm module
	defer func() {
		if r := recover(); r != nil {
			resp = WitResponse{
				Status:  500,
				Headers: []WitHeader{{Name: "Content-Type", Value: "text/plain"}},
				Body:    []byte(fmt.Sprintf("internal server error: %v", r)),
			}
		}
	}()

	handler.ServeHTTP(rc, httpReq)
	return rc.Finish()
}
