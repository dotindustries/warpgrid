// Package wghttp_test exercises the WarpGrid HTTP overlay that bridges
// WIT http-types (http-request / http-response) to Go's net/http interfaces.
//
// Tests are organised by concern:
//   - ConvertRequest: WIT request -> *http.Request
//   - ResponseCapture: http.ResponseWriter -> WIT response
//   - HandleWitRequest: full round-trip through a registered handler
//   - ListenAndServe / HandleFunc / Handle: handler registration
//
// Part of the WarpGrid Go overlay (Domain 3, US-307).
package wghttp_test

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"

	wghttp "github.com/anthropics/warpgrid/packages/warpgrid-go/http"
)

// ── ConvertRequest tests ────────────────────────────────────────────

func TestConvertRequest_BasicGET(t *testing.T) {
	wit := wghttp.WitRequest{
		Method:  "GET",
		URI:     "/users?page=1",
		Headers: nil,
		Body:    nil,
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if req.Method != "GET" {
		t.Fatalf("expected Method=GET, got %s", req.Method)
	}
	if req.URL.Path != "/users" {
		t.Fatalf("expected Path=/users, got %s", req.URL.Path)
	}
	if req.URL.RawQuery != "page=1" {
		t.Fatalf("expected RawQuery=page=1, got %s", req.URL.RawQuery)
	}
	if req.RequestURI != "/users?page=1" {
		t.Fatalf("expected RequestURI=/users?page=1, got %s", req.RequestURI)
	}
	if req.Proto != "HTTP/1.1" {
		t.Fatalf("expected Proto=HTTP/1.1, got %s", req.Proto)
	}
}

func TestConvertRequest_WithHeaders(t *testing.T) {
	wit := wghttp.WitRequest{
		Method: "POST",
		URI:    "/api/data",
		Headers: []wghttp.WitHeader{
			{Name: "Content-Type", Value: "application/json"},
			{Name: "Authorization", Value: "Bearer tok123"},
			{Name: "Accept", Value: "application/json"},
			{Name: "Accept", Value: "text/plain"}, // duplicate header name
		},
		Body: nil,
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if got := req.Header.Get("Content-Type"); got != "application/json" {
		t.Fatalf("Content-Type: expected 'application/json', got '%s'", got)
	}
	if got := req.Header.Get("Authorization"); got != "Bearer tok123" {
		t.Fatalf("Authorization: expected 'Bearer tok123', got '%s'", got)
	}

	// Duplicate Accept header: both values must be present
	accepts := req.Header.Values("Accept")
	if len(accepts) != 2 {
		t.Fatalf("expected 2 Accept values, got %d: %v", len(accepts), accepts)
	}
}

func TestConvertRequest_WithBody(t *testing.T) {
	bodyBytes := []byte(`{"name":"alice"}`)
	wit := wghttp.WitRequest{
		Method: "POST",
		URI:    "/users",
		Headers: []wghttp.WitHeader{
			{Name: "Content-Type", Value: "application/json"},
		},
		Body: bodyBytes,
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if req.ContentLength != int64(len(bodyBytes)) {
		t.Fatalf("ContentLength: expected %d, got %d", len(bodyBytes), req.ContentLength)
	}

	// Body must be readable via io.Reader (streaming interface)
	got, err := io.ReadAll(req.Body)
	if err != nil {
		t.Fatalf("reading body: %v", err)
	}
	if !bytes.Equal(got, bodyBytes) {
		t.Fatalf("body: expected %q, got %q", bodyBytes, got)
	}
}

func TestConvertRequest_StreamingBodyViaReader(t *testing.T) {
	// Verify body is available as io.Reader for streaming reads
	payload := bytes.Repeat([]byte("chunk"), 200) // 1000 bytes
	wit := wghttp.WitRequest{
		Method: "PUT",
		URI:    "/upload",
		Body:   payload,
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	// Read in small chunks to simulate streaming
	var total int
	buf := make([]byte, 64)
	for {
		n, err := req.Body.Read(buf)
		total += n
		if err == io.EOF {
			break
		}
		if err != nil {
			t.Fatalf("streaming read error: %v", err)
		}
	}

	if total != len(payload) {
		t.Fatalf("streamed %d bytes, expected %d", total, len(payload))
	}
}

func TestConvertRequest_EmptyBody(t *testing.T) {
	wit := wghttp.WitRequest{
		Method: "GET",
		URI:    "/health",
		Body:   nil,
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if req.ContentLength != 0 {
		t.Fatalf("ContentLength: expected 0, got %d", req.ContentLength)
	}

	// Body should still be non-nil and readable (returns EOF immediately)
	if req.Body == nil {
		t.Fatal("Body should not be nil")
	}
	got, err := io.ReadAll(req.Body)
	if err != nil {
		t.Fatalf("reading empty body: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("expected empty body, got %d bytes", len(got))
	}
}

func TestConvertRequest_HostFromHeaders(t *testing.T) {
	wit := wghttp.WitRequest{
		Method: "GET",
		URI:    "/",
		Headers: []wghttp.WitHeader{
			{Name: "Host", Value: "api.example.com"},
		},
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if req.Host != "api.example.com" {
		t.Fatalf("Host: expected 'api.example.com', got '%s'", req.Host)
	}
}

func TestConvertRequest_HostFromURI(t *testing.T) {
	wit := wghttp.WitRequest{
		Method: "GET",
		URI:    "http://myhost.local:8080/path",
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if req.Host != "myhost.local:8080" {
		t.Fatalf("Host: expected 'myhost.local:8080', got '%s'", req.Host)
	}
}

func TestConvertRequest_InvalidURI(t *testing.T) {
	wit := wghttp.WitRequest{
		Method: "GET",
		URI:    "://invalid",
	}

	_, err := wghttp.ConvertRequest(wit)
	if err == nil {
		t.Fatal("expected error for invalid URI")
	}
}

// ── ResponseCapture tests ───────────────────────────────────────────

func TestResponseCapture_DefaultStatus(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	// Write body without calling WriteHeader first
	rc.Write([]byte("ok"))

	resp := rc.Finish()
	if resp.Status != 200 {
		t.Fatalf("default status: expected 200, got %d", resp.Status)
	}
}

func TestResponseCapture_WriteHeader_StatusCodes(t *testing.T) {
	codes := []int{200, 201, 400, 404, 500}
	for _, code := range codes {
		t.Run(fmt.Sprintf("status_%d", code), func(t *testing.T) {
			rc := wghttp.NewResponseCapture()
			rc.WriteHeader(code)

			resp := rc.Finish()
			if resp.Status != uint16(code) {
				t.Fatalf("status: expected %d, got %d", code, resp.Status)
			}
		})
	}
}

func TestResponseCapture_Headers(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	rc.Header().Set("Content-Type", "application/json")
	rc.Header().Set("X-Custom", "value")
	rc.Header().Add("X-Multi", "a")
	rc.Header().Add("X-Multi", "b")
	rc.Write([]byte("{}"))

	resp := rc.Finish()

	// Build a lookup map from response headers
	headerMap := make(map[string][]string)
	for _, h := range resp.Headers {
		headerMap[h.Name] = append(headerMap[h.Name], h.Value)
	}

	if got := headerMap["Content-Type"]; len(got) != 1 || got[0] != "application/json" {
		t.Fatalf("Content-Type: expected [application/json], got %v", got)
	}
	if got := headerMap["X-Custom"]; len(got) != 1 || got[0] != "value" {
		t.Fatalf("X-Custom: expected [value], got %v", got)
	}
	if got := headerMap["X-Multi"]; len(got) != 2 {
		t.Fatalf("X-Multi: expected 2 values, got %d: %v", len(got), got)
	}
}

func TestResponseCapture_WriteBody(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	n, err := rc.Write([]byte("hello world"))
	if err != nil {
		t.Fatalf("Write failed: %v", err)
	}
	if n != 11 {
		t.Fatalf("Write returned %d, expected 11", n)
	}

	resp := rc.Finish()
	if !bytes.Equal(resp.Body, []byte("hello world")) {
		t.Fatalf("body: expected 'hello world', got '%s'", resp.Body)
	}
}

func TestResponseCapture_MultipleWrites(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	rc.Write([]byte("part1"))
	rc.Write([]byte("part2"))
	rc.Write([]byte("part3"))

	resp := rc.Finish()
	if !bytes.Equal(resp.Body, []byte("part1part2part3")) {
		t.Fatalf("body: expected 'part1part2part3', got '%s'", resp.Body)
	}
}

func TestResponseCapture_WriteHeaderIgnoredAfterWrite(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	rc.Write([]byte("data")) // implicit 200
	rc.WriteHeader(404)       // should be ignored

	resp := rc.Finish()
	if resp.Status != 200 {
		t.Fatalf("status should remain 200 after WriteHeader post-Write, got %d", resp.Status)
	}
}

func TestResponseCapture_WriteHeaderSetOnce(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	rc.WriteHeader(201)
	rc.WriteHeader(500) // second call should be ignored

	resp := rc.Finish()
	if resp.Status != 201 {
		t.Fatalf("status should remain 201, got %d", resp.Status)
	}
}

func TestResponseCapture_EmptyBody(t *testing.T) {
	rc := wghttp.NewResponseCapture()
	rc.WriteHeader(204)

	resp := rc.Finish()
	if resp.Status != 204 {
		t.Fatalf("status: expected 204, got %d", resp.Status)
	}
	if len(resp.Body) != 0 {
		t.Fatalf("expected empty body, got %d bytes", len(resp.Body))
	}
}

// ── HandleWitRequest round-trip tests ───────────────────────────────

func TestHandleWitRequest_BasicHandler(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain")
		w.WriteHeader(200)
		w.Write([]byte("ok"))
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/health",
	})

	if resp.Status != 200 {
		t.Fatalf("status: expected 200, got %d", resp.Status)
	}
	if !bytes.Equal(resp.Body, []byte("ok")) {
		t.Fatalf("body: expected 'ok', got '%s'", resp.Body)
	}
}

func TestHandleWitRequest_NoHandler(t *testing.T) {
	wghttp.ResetHandler()

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/",
	})

	if resp.Status != 500 {
		t.Fatalf("status: expected 500, got %d", resp.Status)
	}
	if !strings.Contains(string(resp.Body), "no handler registered") {
		t.Fatalf("body should mention 'no handler registered', got '%s'", resp.Body)
	}
}

func TestHandleWitRequest_StatusCodes(t *testing.T) {
	codes := []int{200, 201, 400, 404, 500}
	for _, code := range codes {
		t.Run(fmt.Sprintf("status_%d", code), func(t *testing.T) {
			handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(code)
			})

			wghttp.SetHandler(handler)
			defer wghttp.ResetHandler()

			resp := wghttp.HandleWitRequest(wghttp.WitRequest{
				Method: "GET",
				URI:    "/",
			})

			if resp.Status != uint16(code) {
				t.Fatalf("status: expected %d, got %d", code, resp.Status)
			}
		})
	}
}

func TestHandleWitRequest_EchoBody(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, err := io.ReadAll(r.Body)
		if err != nil {
			w.WriteHeader(500)
			w.Write([]byte(err.Error()))
			return
		}
		w.Header().Set("Content-Type", r.Header.Get("Content-Type"))
		w.Write(body)
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	payload := []byte(`{"key":"value"}`)
	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "POST",
		URI:    "/echo",
		Headers: []wghttp.WitHeader{
			{Name: "Content-Type", Value: "application/json"},
		},
		Body: payload,
	})

	if resp.Status != 200 {
		t.Fatalf("status: expected 200, got %d", resp.Status)
	}
	if !bytes.Equal(resp.Body, payload) {
		t.Fatalf("body: expected %q, got %q", payload, resp.Body)
	}

	// Verify Content-Type header round-tripped
	found := false
	for _, h := range resp.Headers {
		if h.Name == "Content-Type" && h.Value == "application/json" {
			found = true
			break
		}
	}
	if !found {
		t.Fatal("Content-Type header not found in response")
	}
}

func TestHandleWitRequest_StreamingBodyRead(t *testing.T) {
	// Handler reads body in small chunks to exercise io.Reader interface
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var total int
		buf := make([]byte, 16)
		for {
			n, err := r.Body.Read(buf)
			total += n
			if err == io.EOF {
				break
			}
			if err != nil {
				w.WriteHeader(500)
				w.Write([]byte(err.Error()))
				return
			}
		}
		w.Write([]byte(fmt.Sprintf("read %d bytes", total)))
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	payload := bytes.Repeat([]byte("x"), 256)
	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "POST",
		URI:    "/stream",
		Body:   payload,
	})

	if resp.Status != 200 {
		t.Fatalf("status: expected 200, got %d", resp.Status)
	}
	if string(resp.Body) != "read 256 bytes" {
		t.Fatalf("body: expected 'read 256 bytes', got '%s'", resp.Body)
	}
}

func TestHandleWitRequest_JSONRoundTrip(t *testing.T) {
	type User struct {
		ID   int    `json:"id"`
		Name string `json:"name"`
	}

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != "POST" {
			w.WriteHeader(405)
			return
		}
		var u User
		if err := json.NewDecoder(r.Body).Decode(&u); err != nil {
			w.WriteHeader(400)
			w.Write([]byte(err.Error()))
			return
		}
		u.ID = 42
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(201)
		json.NewEncoder(w).Encode(u)
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "POST",
		URI:    "/users",
		Headers: []wghttp.WitHeader{
			{Name: "Content-Type", Value: "application/json"},
		},
		Body: []byte(`{"name":"alice"}`),
	})

	if resp.Status != 201 {
		t.Fatalf("status: expected 201, got %d", resp.Status)
	}

	var got User
	if err := json.Unmarshal(resp.Body, &got); err != nil {
		t.Fatalf("unmarshal response: %v", err)
	}
	if got.ID != 42 || got.Name != "alice" {
		t.Fatalf("unexpected user: %+v", got)
	}
}

func TestHandleWitRequest_HandlerAccessesMethodAndPath(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte(r.Method + " " + r.URL.Path))
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "DELETE",
		URI:    "/users/42",
	})

	if string(resp.Body) != "DELETE /users/42" {
		t.Fatalf("expected 'DELETE /users/42', got '%s'", resp.Body)
	}
}

func TestHandleWitRequest_HandlerAccessesQueryParams(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		page := r.URL.Query().Get("page")
		w.Write([]byte("page=" + page))
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/list?page=3",
	})

	if string(resp.Body) != "page=3" {
		t.Fatalf("expected 'page=3', got '%s'", resp.Body)
	}
}

// ── Handler registration tests ──────────────────────────────────────

func TestListenAndServe_RegistersHandler(t *testing.T) {
	wghttp.ResetHandler()

	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("registered"))
	})

	err := wghttp.ListenAndServe(":8080", handler)
	if err != nil {
		t.Fatalf("ListenAndServe failed: %v", err)
	}

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/",
	})

	if string(resp.Body) != "registered" {
		t.Fatalf("expected 'registered', got '%s'", resp.Body)
	}

	wghttp.ResetHandler()
}

func TestListenAndServe_NilHandlerUsesDefaultMux(t *testing.T) {
	wghttp.ResetHandler()

	// Register on the WarpGrid ServeMux
	wghttp.HandleFunc("/wg-test", func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("from default mux"))
	})

	err := wghttp.ListenAndServe(":8080", nil)
	if err != nil {
		t.Fatalf("ListenAndServe failed: %v", err)
	}

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/wg-test",
	})

	if string(resp.Body) != "from default mux" {
		t.Fatalf("expected 'from default mux', got '%s'", resp.Body)
	}

	wghttp.ResetHandler()
	wghttp.ResetDefaultServeMux()
}

func TestHandleFunc_RegistersOnDefaultMux(t *testing.T) {
	wghttp.ResetHandler()
	wghttp.ResetDefaultServeMux()

	wghttp.HandleFunc("/hello", func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("hello from HandleFunc"))
	})

	// Must call ListenAndServe with nil to use default mux
	wghttp.ListenAndServe(":0", nil)

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/hello",
	})

	if string(resp.Body) != "hello from HandleFunc" {
		t.Fatalf("expected 'hello from HandleFunc', got '%s'", resp.Body)
	}

	wghttp.ResetHandler()
	wghttp.ResetDefaultServeMux()
}

func TestHandle_RegistersOnDefaultMux(t *testing.T) {
	wghttp.ResetHandler()
	wghttp.ResetDefaultServeMux()

	wghttp.Handle("/custom", http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Write([]byte("custom handler"))
	}))

	wghttp.ListenAndServe(":0", nil)

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/custom",
	})

	if string(resp.Body) != "custom handler" {
		t.Fatalf("expected 'custom handler', got '%s'", resp.Body)
	}

	wghttp.ResetHandler()
	wghttp.ResetDefaultServeMux()
}

// ── Edge cases ──────────────────────────────────────────────────────

func TestHandleWitRequest_LargeBody(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		w.Write([]byte(fmt.Sprintf("received %d bytes", len(body))))
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	largeBody := bytes.Repeat([]byte("a"), 1<<20) // 1 MiB
	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "POST",
		URI:    "/upload",
		Body:   largeBody,
	})

	expected := fmt.Sprintf("received %d bytes", 1<<20)
	if string(resp.Body) != expected {
		t.Fatalf("expected '%s', got '%s'", expected, resp.Body)
	}
}

func TestHandleWitRequest_HandlerPanic(t *testing.T) {
	handler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		panic("handler panic")
	})

	wghttp.SetHandler(handler)
	defer wghttp.ResetHandler()

	resp := wghttp.HandleWitRequest(wghttp.WitRequest{
		Method: "GET",
		URI:    "/panic",
	})

	// Should recover from panic and return 500
	if resp.Status != 500 {
		t.Fatalf("status: expected 500 after panic, got %d", resp.Status)
	}
	if !strings.Contains(string(resp.Body), "internal server error") {
		t.Fatalf("body should indicate internal error, got '%s'", resp.Body)
	}
}

func TestConvertRequest_EmptyHeaders(t *testing.T) {
	wit := wghttp.WitRequest{
		Method:  "GET",
		URI:     "/",
		Headers: []wghttp.WitHeader{},
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}
	if len(req.Header) != 0 {
		t.Fatalf("expected no headers, got %d", len(req.Header))
	}
}

func TestConvertRequest_ZeroLengthBody(t *testing.T) {
	wit := wghttp.WitRequest{
		Method: "POST",
		URI:    "/empty",
		Body:   []byte{},
	}

	req, err := wghttp.ConvertRequest(wit)
	if err != nil {
		t.Fatalf("ConvertRequest failed: %v", err)
	}

	if req.ContentLength != 0 {
		t.Fatalf("ContentLength: expected 0, got %d", req.ContentLength)
	}

	got, _ := io.ReadAll(req.Body)
	if len(got) != 0 {
		t.Fatalf("expected empty body, got %d bytes", len(got))
	}
}
