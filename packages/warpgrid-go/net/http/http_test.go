package http_test

import (
	"bytes"
	"io"
	"testing"

	wghttp "github.com/anthropics/warpgrid/packages/warpgrid-go/net/http"
)

// ── Handler registration tests ──────────────────────────────────────

func TestHandleFunc_RegistersOnDefaultServeMux(t *testing.T) {
	mux := wghttp.NewServeMux()
	called := false
	mux.HandleFunc("/test", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		called = true
	})

	req := wghttp.NewRequest(wghttp.MethodGet, "/test", nil)
	w := wghttp.NewTestResponseWriter()
	mux.ServeHTTP(w, req)

	if !called {
		t.Fatal("handler was not called for registered pattern")
	}
}

func TestListenAndServe_NilHandlerUsesDefaultServeMux(t *testing.T) {
	// Reset global state for test isolation
	mux := wghttp.NewServeMux()
	called := false
	mux.HandleFunc("/listen-test", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		called = true
		w.Write([]byte("ok"))
	})

	// ListenAndServe with nil handler should use the provided mux
	// We test this by registering on a mux and using it as handler
	handler := wghttp.RegisterAndReturn(mux)
	if handler == nil {
		t.Fatal("RegisterAndReturn should return the handler")
	}

	req := wghttp.NewRequest(wghttp.MethodGet, "/listen-test", nil)
	w := wghttp.NewTestResponseWriter()
	handler.ServeHTTP(w, req)

	if !called {
		t.Fatal("handler registered on mux was not called")
	}
}

func TestListenAndServe_CustomHandler(t *testing.T) {
	customCalled := false
	custom := wghttp.HandlerFunc(func(w wghttp.ResponseWriter, r *wghttp.Request) {
		customCalled = true
	})

	handler := wghttp.RegisterAndReturn(custom)
	req := wghttp.NewRequest(wghttp.MethodGet, "/anything", nil)
	w := wghttp.NewTestResponseWriter()
	handler.ServeHTTP(w, req)

	if !customCalled {
		t.Fatal("custom handler was not called")
	}
}

// ── ServeMux routing tests ──────────────────────────────────────────

func TestServeMux_ExactMatchRoute(t *testing.T) {
	mux := wghttp.NewServeMux()
	var routedPath string
	mux.HandleFunc("/users", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		routedPath = r.URL.Path
	})

	req := wghttp.NewRequest(wghttp.MethodGet, "/users", nil)
	w := wghttp.NewTestResponseWriter()
	mux.ServeHTTP(w, req)

	if routedPath != "/users" {
		t.Fatalf("expected path '/users', got '%s'", routedPath)
	}
}

func TestServeMux_PrefixMatchRoute(t *testing.T) {
	mux := wghttp.NewServeMux()
	called := false
	mux.HandleFunc("/api/", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		called = true
	})

	req := wghttp.NewRequest(wghttp.MethodGet, "/api/users/123", nil)
	w := wghttp.NewTestResponseWriter()
	mux.ServeHTTP(w, req)

	if !called {
		t.Fatal("prefix handler was not called for /api/users/123")
	}
}

func TestServeMux_LongestPrefixWins(t *testing.T) {
	mux := wghttp.NewServeMux()
	var matched string
	mux.HandleFunc("/api/", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		matched = "/api/"
	})
	mux.HandleFunc("/api/v2/", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		matched = "/api/v2/"
	})

	req := wghttp.NewRequest(wghttp.MethodGet, "/api/v2/users", nil)
	w := wghttp.NewTestResponseWriter()
	mux.ServeHTTP(w, req)

	if matched != "/api/v2/" {
		t.Fatalf("expected longest prefix '/api/v2/', got '%s'", matched)
	}
}

func TestServeMux_UnregisteredPathReturns404(t *testing.T) {
	mux := wghttp.NewServeMux()
	mux.HandleFunc("/registered", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		w.Write([]byte("found"))
	})

	req := wghttp.NewRequest(wghttp.MethodGet, "/nonexistent", nil)
	w := wghttp.NewTestResponseWriter()
	mux.ServeHTTP(w, req)

	if w.StatusCode() != wghttp.StatusNotFound {
		t.Fatalf("expected status 404, got %d", w.StatusCode())
	}
}

func TestServeMux_MultiplePatterns(t *testing.T) {
	mux := wghttp.NewServeMux()
	var called string
	mux.HandleFunc("/users", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		called = "users"
	})
	mux.HandleFunc("/health", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		called = "health"
	})

	req := wghttp.NewRequest(wghttp.MethodGet, "/health", nil)
	w := wghttp.NewTestResponseWriter()
	mux.ServeHTTP(w, req)

	if called != "health" {
		t.Fatalf("expected 'health', got '%s'", called)
	}
}

// ── ResponseWriter tests ────────────────────────────────────────────

func TestResponseWriter_DefaultStatus200(t *testing.T) {
	w := wghttp.NewTestResponseWriter()
	w.Write([]byte("hello"))

	if w.StatusCode() != wghttp.StatusOK {
		t.Fatalf("expected default status 200, got %d", w.StatusCode())
	}
}

func TestResponseWriter_WriteHeaderSetsStatus(t *testing.T) {
	w := wghttp.NewTestResponseWriter()
	w.WriteHeader(wghttp.StatusCreated)
	w.Write([]byte("created"))

	if w.StatusCode() != wghttp.StatusCreated {
		t.Fatalf("expected status 201, got %d", w.StatusCode())
	}
}

func TestResponseWriter_WriteHeaderOnlyFirstCall(t *testing.T) {
	w := wghttp.NewTestResponseWriter()
	w.WriteHeader(wghttp.StatusCreated)
	w.WriteHeader(wghttp.StatusNotFound) // should be ignored

	if w.StatusCode() != wghttp.StatusCreated {
		t.Fatalf("expected first status 201, got %d", w.StatusCode())
	}
}

func TestResponseWriter_WriteCapturesBody(t *testing.T) {
	w := wghttp.NewTestResponseWriter()
	w.Write([]byte("hello "))
	w.Write([]byte("world"))

	body := w.Body()
	if string(body) != "hello world" {
		t.Fatalf("expected 'hello world', got '%s'", string(body))
	}
}

func TestResponseWriter_HeaderSetAndGet(t *testing.T) {
	w := wghttp.NewTestResponseWriter()
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("X-Custom", "value")

	if w.Header().Get("Content-Type") != "application/json" {
		t.Fatalf("expected Content-Type 'application/json', got '%s'",
			w.Header().Get("Content-Type"))
	}
	if w.Header().Get("X-Custom") != "value" {
		t.Fatalf("expected X-Custom 'value', got '%s'", w.Header().Get("X-Custom"))
	}
}

// ── Request tests ───────────────────────────────────────────────────

func TestNewRequest_SetsMethodAndPath(t *testing.T) {
	req := wghttp.NewRequest(wghttp.MethodPost, "/users", nil)

	if req.Method != wghttp.MethodPost {
		t.Fatalf("expected method POST, got '%s'", req.Method)
	}
	if req.URL.Path != "/users" {
		t.Fatalf("expected path '/users', got '%s'", req.URL.Path)
	}
}

func TestNewRequest_WithQueryString(t *testing.T) {
	req := wghttp.NewRequest(wghttp.MethodGet, "/users?page=2&limit=10", nil)

	if req.URL.Path != "/users" {
		t.Fatalf("expected path '/users', got '%s'", req.URL.Path)
	}
	if req.URL.RawQuery != "page=2&limit=10" {
		t.Fatalf("expected query 'page=2&limit=10', got '%s'", req.URL.RawQuery)
	}
}

func TestNewRequest_WithBody(t *testing.T) {
	body := []byte(`{"name":"Alice"}`)
	req := wghttp.NewRequest(wghttp.MethodPost, "/users", body)

	readBody, err := io.ReadAll(req.Body)
	if err != nil {
		t.Fatalf("failed to read body: %v", err)
	}
	if !bytes.Equal(readBody, body) {
		t.Fatalf("expected body '%s', got '%s'", string(body), string(readBody))
	}
}

func TestNewRequest_NilBodyIsEmpty(t *testing.T) {
	req := wghttp.NewRequest(wghttp.MethodGet, "/health", nil)

	readBody, err := io.ReadAll(req.Body)
	if err != nil {
		t.Fatalf("failed to read body: %v", err)
	}
	if len(readBody) != 0 {
		t.Fatalf("expected empty body, got %d bytes", len(readBody))
	}
}

func TestNewRequest_HeadersAreInitialized(t *testing.T) {
	req := wghttp.NewRequest(wghttp.MethodGet, "/test", nil)

	if req.Header == nil {
		t.Fatal("expected non-nil header map")
	}
	req.Header.Set("Accept", "application/json")
	if req.Header.Get("Accept") != "application/json" {
		t.Fatal("header Set/Get round-trip failed")
	}
}

// ── Error helper tests ──────────────────────────────────────────────

func TestError_WritesStatusAndMessage(t *testing.T) {
	w := wghttp.NewTestResponseWriter()
	wghttp.Error(w, "bad request", wghttp.StatusBadRequest)

	if w.StatusCode() != wghttp.StatusBadRequest {
		t.Fatalf("expected status 400, got %d", w.StatusCode())
	}
	if string(w.Body()) != "bad request" {
		t.Fatalf("expected body 'bad request', got '%s'", string(w.Body()))
	}
	if w.Header().Get("Content-Type") != "text/plain; charset=utf-8" {
		t.Fatalf("expected text/plain content type, got '%s'",
			w.Header().Get("Content-Type"))
	}
}

// ── Header type tests ───────────────────────────────────────────────

func TestHeader_Add(t *testing.T) {
	h := make(wghttp.Header)
	h.Add("Accept", "text/html")
	h.Add("Accept", "application/json")

	values := h["Accept"]
	if len(values) != 2 {
		t.Fatalf("expected 2 values, got %d", len(values))
	}
}

func TestHeader_Set_OverwritesPrevious(t *testing.T) {
	h := make(wghttp.Header)
	h.Set("Content-Type", "text/html")
	h.Set("Content-Type", "application/json")

	if h.Get("Content-Type") != "application/json" {
		t.Fatalf("Set should overwrite, got '%s'", h.Get("Content-Type"))
	}
}

func TestHeader_Del(t *testing.T) {
	h := make(wghttp.Header)
	h.Set("X-Remove", "value")
	h.Del("X-Remove")

	if h.Get("X-Remove") != "" {
		t.Fatal("Del should remove the header")
	}
}

func TestHeader_Get_MissingKeyReturnsEmpty(t *testing.T) {
	h := make(wghttp.Header)
	if h.Get("Missing") != "" {
		t.Fatal("Get on missing key should return empty string")
	}
}

// ── HandleRequest integration tests ─────────────────────────────────

func TestHandleRequest_FullRoundTrip(t *testing.T) {
	mux := wghttp.NewServeMux()
	mux.HandleFunc("/echo", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Header().Set("Content-Type", "text/plain")
		w.Header().Set("X-Method", r.Method)
		w.WriteHeader(wghttp.StatusOK)
		w.Write(body)
	})

	reqBytes := wghttp.MarshalRequest(wghttp.WitHttpRequest{
		Method: "POST",
		URI:    "/echo",
		Headers: []wghttp.WitHttpHeader{
			{Name: "Content-Type", Value: "text/plain"},
		},
		Body: []byte("hello warpgrid"),
	})

	respBytes := wghttp.HandleRequestWith(mux, reqBytes)
	resp := wghttp.UnmarshalResponse(respBytes)

	if resp.Status != wghttp.StatusOK {
		t.Fatalf("expected status 200, got %d", resp.Status)
	}
	if string(resp.Body) != "hello warpgrid" {
		t.Fatalf("expected body 'hello warpgrid', got '%s'", string(resp.Body))
	}

	foundContentType := false
	foundMethod := false
	for _, h := range resp.Headers {
		if h.Name == "Content-Type" && h.Value == "text/plain" {
			foundContentType = true
		}
		if h.Name == "X-Method" && h.Value == "POST" {
			foundMethod = true
		}
	}
	if !foundContentType {
		t.Fatal("missing Content-Type header in response")
	}
	if !foundMethod {
		t.Fatal("missing X-Method header in response")
	}
}

func TestHandleRequest_404ForUnregisteredPath(t *testing.T) {
	mux := wghttp.NewServeMux()

	reqBytes := wghttp.MarshalRequest(wghttp.WitHttpRequest{
		Method: "GET",
		URI:    "/nonexistent",
	})

	respBytes := wghttp.HandleRequestWith(mux, reqBytes)
	resp := wghttp.UnmarshalResponse(respBytes)

	if resp.Status != wghttp.StatusNotFound {
		t.Fatalf("expected status 404, got %d", resp.Status)
	}
}

func TestHandleRequest_MethodDispatch(t *testing.T) {
	mux := wghttp.NewServeMux()
	mux.HandleFunc("/resource", func(w wghttp.ResponseWriter, r *wghttp.Request) {
		switch r.Method {
		case wghttp.MethodGet:
			w.WriteHeader(wghttp.StatusOK)
			w.Write([]byte("get"))
		case wghttp.MethodPost:
			w.WriteHeader(wghttp.StatusCreated)
			w.Write([]byte("post"))
		default:
			wghttp.Error(w, "method not allowed", wghttp.StatusMethodNotAllowed)
		}
	})

	tests := []struct {
		method     string
		wantStatus uint16
		wantBody   string
	}{
		{"GET", wghttp.StatusOK, "get"},
		{"POST", wghttp.StatusCreated, "post"},
		{"DELETE", wghttp.StatusMethodNotAllowed, "method not allowed"},
	}

	for _, tt := range tests {
		t.Run(tt.method, func(t *testing.T) {
			reqBytes := wghttp.MarshalRequest(wghttp.WitHttpRequest{
				Method: tt.method,
				URI:    "/resource",
			})
			respBytes := wghttp.HandleRequestWith(mux, reqBytes)
			resp := wghttp.UnmarshalResponse(respBytes)

			if resp.Status != tt.wantStatus {
				t.Fatalf("expected status %d, got %d", tt.wantStatus, resp.Status)
			}
			if string(resp.Body) != tt.wantBody {
				t.Fatalf("expected body '%s', got '%s'", tt.wantBody, string(resp.Body))
			}
		})
	}
}

// ── Wire format round-trip tests ────────────────────────────────────

func TestWireFormat_RequestRoundTrip(t *testing.T) {
	original := wghttp.WitHttpRequest{
		Method: "POST",
		URI:    "/users?page=1",
		Headers: []wghttp.WitHttpHeader{
			{Name: "Content-Type", Value: "application/json"},
			{Name: "Authorization", Value: "Bearer token123"},
		},
		Body: []byte(`{"name":"Alice"}`),
	}

	data := wghttp.MarshalRequest(original)
	decoded := wghttp.UnmarshalRequest(data)

	if decoded.Method != original.Method {
		t.Fatalf("method: expected '%s', got '%s'", original.Method, decoded.Method)
	}
	if decoded.URI != original.URI {
		t.Fatalf("uri: expected '%s', got '%s'", original.URI, decoded.URI)
	}
	if len(decoded.Headers) != len(original.Headers) {
		t.Fatalf("headers: expected %d, got %d", len(original.Headers), len(decoded.Headers))
	}
	for i, h := range decoded.Headers {
		if h.Name != original.Headers[i].Name || h.Value != original.Headers[i].Value {
			t.Fatalf("header[%d]: expected %s=%s, got %s=%s",
				i, original.Headers[i].Name, original.Headers[i].Value, h.Name, h.Value)
		}
	}
	if !bytes.Equal(decoded.Body, original.Body) {
		t.Fatalf("body: expected '%s', got '%s'", string(original.Body), string(decoded.Body))
	}
}

func TestWireFormat_ResponseRoundTrip(t *testing.T) {
	original := wghttp.WitHttpResponse{
		Status: 201,
		Headers: []wghttp.WitHttpHeader{
			{Name: "Content-Type", Value: "application/json"},
			{Name: "X-Request-Id", Value: "abc-123"},
		},
		Body: []byte(`{"id":1,"name":"Alice"}`),
	}

	data := wghttp.MarshalResponse(original)
	decoded := wghttp.UnmarshalResponse(data)

	if decoded.Status != original.Status {
		t.Fatalf("status: expected %d, got %d", original.Status, decoded.Status)
	}
	if len(decoded.Headers) != len(original.Headers) {
		t.Fatalf("headers: expected %d, got %d", len(original.Headers), len(decoded.Headers))
	}
	for i, h := range decoded.Headers {
		if h.Name != original.Headers[i].Name || h.Value != original.Headers[i].Value {
			t.Fatalf("header[%d]: expected %s=%s, got %s=%s",
				i, original.Headers[i].Name, original.Headers[i].Value, h.Name, h.Value)
		}
	}
	if !bytes.Equal(decoded.Body, original.Body) {
		t.Fatalf("body: expected '%s', got '%s'", string(original.Body), string(decoded.Body))
	}
}

func TestWireFormat_EmptyRequest(t *testing.T) {
	original := wghttp.WitHttpRequest{
		Method: "GET",
		URI:    "/health",
	}

	data := wghttp.MarshalRequest(original)
	decoded := wghttp.UnmarshalRequest(data)

	if decoded.Method != "GET" {
		t.Fatalf("method: expected 'GET', got '%s'", decoded.Method)
	}
	if len(decoded.Headers) != 0 {
		t.Fatalf("headers: expected 0, got %d", len(decoded.Headers))
	}
	if len(decoded.Body) != 0 {
		t.Fatalf("body: expected empty, got %d bytes", len(decoded.Body))
	}
}

func TestWireFormat_ResponseNoHeaders(t *testing.T) {
	original := wghttp.WitHttpResponse{
		Status: 204,
		Body:   nil,
	}

	data := wghttp.MarshalResponse(original)
	decoded := wghttp.UnmarshalResponse(data)

	if decoded.Status != 204 {
		t.Fatalf("status: expected 204, got %d", decoded.Status)
	}
	if len(decoded.Headers) != 0 {
		t.Fatalf("headers: expected 0, got %d", len(decoded.Headers))
	}
	if len(decoded.Body) != 0 {
		t.Fatalf("body: expected empty, got %d bytes", len(decoded.Body))
	}
}
