package wghttp

import (
	"bytes"
	"net/http"
)

// ResponseCapture implements http.ResponseWriter by capturing all writes
// into an in-memory buffer. After the handler returns, call Finish() to
// extract a WitResponse.
//
// Behavior matches net/http semantics:
//   - Default status is 200 (sent implicitly on first Write)
//   - WriteHeader can only be called once; subsequent calls are ignored
//   - Write triggers an implicit WriteHeader(200) if not already called
type ResponseCapture struct {
	status      int
	headers     http.Header
	body        bytes.Buffer
	headersSent bool
}

// NewResponseCapture creates a ResponseCapture with default 200 status
// and empty headers.
func NewResponseCapture() *ResponseCapture {
	return &ResponseCapture{
		status:  200,
		headers: make(http.Header),
	}
}

// Header returns the response header map. Headers set before WriteHeader
// or the first Write call are included in the WIT response.
func (rc *ResponseCapture) Header() http.Header {
	return rc.headers
}

// Write writes the data to the response body buffer. If WriteHeader has
// not been called, an implicit WriteHeader(200) is triggered.
func (rc *ResponseCapture) Write(data []byte) (int, error) {
	if !rc.headersSent {
		rc.headersSent = true
	}
	return rc.body.Write(data)
}

// WriteHeader sends an HTTP response header with the provided status code.
// Only the first call takes effect; subsequent calls are no-ops matching
// net/http behavior.
func (rc *ResponseCapture) WriteHeader(statusCode int) {
	if rc.headersSent {
		return
	}
	rc.status = statusCode
	rc.headersSent = true
}

// Finish extracts the captured response as a WitResponse. This should be
// called after the handler has returned.
func (rc *ResponseCapture) Finish() WitResponse {
	var witHeaders []WitHeader
	for name, values := range rc.headers {
		for _, v := range values {
			witHeaders = append(witHeaders, WitHeader{Name: name, Value: v})
		}
	}

	return WitResponse{
		Status:  uint16(rc.status),
		Headers: witHeaders,
		Body:    rc.body.Bytes(),
	}
}
