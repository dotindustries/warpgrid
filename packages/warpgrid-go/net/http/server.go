package http

import "sync"

// ServeMux is an HTTP request multiplexer matching registered patterns
// against the request URL path. Exact matches take priority; trailing-
// slash patterns match as prefixes (longest match wins).
type ServeMux struct {
	mu       sync.RWMutex
	handlers map[string]Handler
}

// NewServeMux creates a new ServeMux.
func NewServeMux() *ServeMux {
	return &ServeMux{
		handlers: make(map[string]Handler),
	}
}

// Handle registers the handler for the given pattern.
func (mux *ServeMux) Handle(pattern string, handler Handler) {
	mux.mu.Lock()
	defer mux.mu.Unlock()
	mux.handlers[pattern] = handler
}

// HandleFunc registers the handler function for the given pattern.
func (mux *ServeMux) HandleFunc(pattern string, handler func(ResponseWriter, *Request)) {
	mux.Handle(pattern, HandlerFunc(handler))
}

// ServeHTTP dispatches the request to the handler whose pattern
// matches the request URL path.
func (mux *ServeMux) ServeHTTP(w ResponseWriter, r *Request) {
	mux.mu.RLock()
	defer mux.mu.RUnlock()

	path := r.URL.Path

	// Exact match first
	if h, ok := mux.handlers[path]; ok {
		h.ServeHTTP(w, r)
		return
	}

	// Prefix match: trailing-slash patterns, longest match wins
	var bestPattern string
	var bestHandler Handler
	for pattern, handler := range mux.handlers {
		if len(pattern) > 0 && pattern[len(pattern)-1] == '/' {
			if len(path) >= len(pattern) && path[:len(pattern)] == pattern {
				if len(pattern) > len(bestPattern) {
					bestPattern = pattern
					bestHandler = handler
				}
			}
		}
	}

	if bestHandler != nil {
		bestHandler.ServeHTTP(w, r)
		return
	}

	Error(w, "404 page not found", StatusNotFound)
}

// DefaultServeMux is the default ServeMux used by HandleFunc and
// ListenAndServe when handler is nil.
var DefaultServeMux = NewServeMux()

// HandleFunc registers the handler function for the given pattern
// on DefaultServeMux.
func HandleFunc(pattern string, handler func(ResponseWriter, *Request)) {
	DefaultServeMux.HandleFunc(pattern, handler)
}

// Handle registers the handler for the given pattern on DefaultServeMux.
func Handle(pattern string, handler Handler) {
	DefaultServeMux.Handle(pattern, handler)
}

// registeredHandler holds the handler set by ListenAndServe/Register.
var registeredHandler Handler

// ListenAndServe registers the handler with the WarpGrid trigger system.
//
// Unlike net/http.ListenAndServe, this does NOT open a TCP socket.
// The addr parameter is accepted for API compatibility but has no effect;
// WarpGrid manages the listener configuration externally.
//
// If handler is nil, DefaultServeMux is used.
//
// Returns nil immediately. The WarpGrid runtime invokes the registered
// handler for each inbound HTTP request via HandleRequest.
func ListenAndServe(addr string, handler Handler) error {
	if handler == nil {
		handler = DefaultServeMux
	}
	registeredHandler = handler
	return nil
}

// RegisterAndReturn stores the handler and returns it. This is a test
// helper that allows verifying handler registration without blocking.
func RegisterAndReturn(handler Handler) Handler {
	if handler == nil {
		handler = DefaultServeMux
	}
	registeredHandler = handler
	return registeredHandler
}

// HandleRequest processes a serialized WIT HTTP request through the
// globally registered handler and returns the serialized WIT response.
//
// This is the entry point called by the WASI export bridge. If no
// handler has been registered (ListenAndServe not yet called), it
// returns a 503 Service Unavailable response.
func HandleRequest(reqBytes []byte) []byte {
	if registeredHandler == nil {
		return MarshalResponse(WitHttpResponse{
			Status: StatusServiceUnavailable,
			Headers: []WitHttpHeader{
				{Name: "Content-Type", Value: "text/plain; charset=utf-8"},
			},
			Body: []byte("no handler registered"),
		})
	}
	return HandleRequestWith(registeredHandler, reqBytes)
}

// HandleRequestWith processes a serialized WIT HTTP request through
// the given handler and returns the serialized WIT response.
func HandleRequestWith(handler Handler, reqBytes []byte) []byte {
	witReq := UnmarshalRequest(reqBytes)
	req := witRequestToGoRequest(witReq)

	w := newBufferResponseWriter()
	handler.ServeHTTP(w, req)

	resp := WitHttpResponse{
		Status:  uint16(w.statusCode),
		Headers: goHeadersToWitHeaders(w.header),
		Body:    w.body,
	}
	return MarshalResponse(resp)
}

// witRequestToGoRequest converts a WIT HTTP request to a Go Request.
func witRequestToGoRequest(wit WitHttpRequest) *Request {
	req := NewRequest(wit.Method, wit.URI, wit.Body)
	for _, h := range wit.Headers {
		req.Header.Add(h.Name, h.Value)
	}
	return req
}

// goHeadersToWitHeaders converts Go Header map to WIT header list.
func goHeadersToWitHeaders(h Header) []WitHttpHeader {
	var headers []WitHttpHeader
	for name, values := range h {
		for _, value := range values {
			headers = append(headers, WitHttpHeader{Name: name, Value: value})
		}
	}
	return headers
}
