// Package wghttp provides an HTTP overlay for WarpGrid WASI modules.
//
// It bridges WIT http-types (http-request / http-response) to Go's standard
// net/http interfaces so that Go handlers can process HTTP requests arriving
// through the WarpGrid trigger system.
//
// On WASI targets, ListenAndServe registers the handler with the host's
// async-handler export instead of opening a socket. On standard Go (for
// testing and native development), the handler is stored and invocable
// via HandleWitRequest.
//
// This package is part of the WarpGrid Go overlay (Domain 3, US-306/US-307).
package wghttp

import (
	"bytes"
	"io"
	"net/http"
	"net/url"
)

// WitHeader mirrors the WIT record warpgrid:shim/http-types.http-header.
type WitHeader struct {
	Name  string
	Value string
}

// WitRequest mirrors the WIT record warpgrid:shim/http-types.http-request.
type WitRequest struct {
	Method  string
	URI     string
	Headers []WitHeader
	Body    []byte
}

// WitResponse mirrors the WIT record warpgrid:shim/http-types.http-response.
type WitResponse struct {
	Status  uint16
	Headers []WitHeader
	Body    []byte
}

// ConvertRequest converts a WIT http-request to a Go *http.Request.
//
// The returned request has:
//   - Method, URL, and RequestURI set from the WIT fields
//   - Headers populated from the WIT header list
//   - Body backed by a bytes.Reader (supports io.Reader streaming)
//   - Host set from the "Host" header or the URI authority
//   - Proto set to "HTTP/1.1" (the WIT layer is protocol-agnostic)
func ConvertRequest(wit WitRequest) (*http.Request, error) {
	parsedURL, err := url.ParseRequestURI(wit.URI)
	if err != nil {
		return nil, err
	}

	body := wit.Body
	if body == nil {
		body = []byte{}
	}

	req := &http.Request{
		Method:        wit.Method,
		URL:           parsedURL,
		RequestURI:    wit.URI,
		Proto:         "HTTP/1.1",
		ProtoMajor:    1,
		ProtoMinor:    1,
		Header:        make(http.Header),
		Body:          io.NopCloser(bytes.NewReader(body)),
		ContentLength: int64(len(body)),
		Host:          parsedURL.Host,
	}

	for _, h := range wit.Headers {
		req.Header.Add(h.Name, h.Value)
	}

	// Host header overrides the URI authority
	if host := req.Header.Get("Host"); host != "" {
		req.Host = host
	}

	return req, nil
}
