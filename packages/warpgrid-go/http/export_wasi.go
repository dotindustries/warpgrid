//go:build wasip1 || wasip2

package wghttp

// This file contains the WASI-specific export bridge for the async-handler
// WIT interface. When compiled with TinyGo targeting wasip2, the
// //go:wasmexport directive creates a core module export that the
// component adapter maps to warpgrid:shim/async-handler@0.1.0#handle-request.
//
// The canonical ABI for the complex http-request/http-response records is
// handled through a simplified byte-buffer protocol:
//
//   - Request fields are passed as (ptr, len) pairs for each string/list field
//   - Response is written to caller-allocated memory via a return pointer
//
// The actual ABI layout is determined by the WIT canonical ABI spec and
// matched by the wasm-tools component adapter during `warp pack --lang go`.
//
// For now, the low-level ABI bridge is a placeholder. The full canonical ABI
// marshaling will be implemented as part of US-310 (warp pack --lang go)
// when the component build pipeline connects all the pieces.
//
// Domain 3, US-306/US-307.

import "unsafe"

// handleRequest is the core module export that the component adapter maps
// to the WIT async-handler.handle-request function.
//
// Parameters (canonical ABI flattened http-request):
//
//	methodPtr/methodLen     - HTTP method string
//	uriPtr/uriLen           - Request URI string
//	headersPtr/headersLen   - Serialized headers (name\0value\0 pairs)
//	bodyPtr/bodyLen         - Request body bytes
//	retPtr                  - Pointer to write the response into
//
// Response layout at retPtr (canonical ABI flattened http-response):
//
//	[0:2]   u16 status code
//	[4:8]   ptr to headers data
//	[8:12]  headers data length
//	[12:16] ptr to body data
//	[16:20] body data length
//
//go:wasmexport warpgrid-handle-request
func handleRequest(
	methodPtr *byte, methodLen uint32,
	uriPtr *byte, uriLen uint32,
	headersPtr *byte, headersLen uint32,
	bodyPtr *byte, bodyLen uint32,
	retPtr *byte,
) {
	method := ptrToString(methodPtr, methodLen)
	uri := ptrToString(uriPtr, uriLen)
	body := ptrToBytes(bodyPtr, bodyLen)
	headers := deserializeHeaders(headersPtr, headersLen)

	req := WitRequest{
		Method:  method,
		URI:     uri,
		Headers: headers,
		Body:    body,
	}

	resp := HandleWitRequest(req)
	serializeResponse(resp, retPtr)
}

// ptrToString converts a (ptr, len) pair from the canonical ABI into a Go string.
func ptrToString(ptr *byte, length uint32) string {
	if length == 0 || ptr == nil {
		return ""
	}
	return unsafe.String(ptr, length)
}

// ptrToBytes converts a (ptr, len) pair into a Go byte slice.
func ptrToBytes(ptr *byte, length uint32) []byte {
	if length == 0 || ptr == nil {
		return nil
	}
	return unsafe.Slice(ptr, length)
}

// deserializeHeaders decodes a null-separated header buffer into WitHeader pairs.
// Format: name\0value\0name\0value\0...
func deserializeHeaders(ptr *byte, length uint32) []WitHeader {
	if length == 0 || ptr == nil {
		return nil
	}

	data := unsafe.Slice(ptr, length)
	var headers []WitHeader
	i := 0
	for i < len(data) {
		// Find name end
		nameEnd := i
		for nameEnd < len(data) && data[nameEnd] != 0 {
			nameEnd++
		}
		if nameEnd >= len(data) {
			break
		}
		name := string(data[i:nameEnd])

		// Find value end
		valStart := nameEnd + 1
		valEnd := valStart
		for valEnd < len(data) && data[valEnd] != 0 {
			valEnd++
		}
		value := string(data[valStart:valEnd])

		headers = append(headers, WitHeader{Name: name, Value: value})
		i = valEnd + 1
	}
	return headers
}

// serializeResponse writes a WitResponse to the caller's return buffer.
func serializeResponse(resp WitResponse, retPtr *byte) {
	if retPtr == nil {
		return
	}

	// Serialize headers into null-separated format
	var headerBuf []byte
	for _, h := range resp.Headers {
		headerBuf = append(headerBuf, []byte(h.Name)...)
		headerBuf = append(headerBuf, 0)
		headerBuf = append(headerBuf, []byte(h.Value)...)
		headerBuf = append(headerBuf, 0)
	}

	ret := unsafe.Slice(retPtr, 20)

	// Status (u16 at offset 0)
	ret[0] = byte(resp.Status)
	ret[1] = byte(resp.Status >> 8)
	ret[2] = 0
	ret[3] = 0

	// Headers pointer and length (offsets 4-11)
	if len(headerBuf) > 0 {
		hPtr := unsafe.Pointer(&headerBuf[0])
		writeU32(ret[4:8], uint32(uintptr(hPtr)))
		writeU32(ret[8:12], uint32(len(headerBuf)))
	} else {
		writeU32(ret[4:8], 0)
		writeU32(ret[8:12], 0)
	}

	// Body pointer and length (offsets 12-19)
	if len(resp.Body) > 0 {
		bPtr := unsafe.Pointer(&resp.Body[0])
		writeU32(ret[12:16], uint32(uintptr(bPtr)))
		writeU32(ret[16:20], uint32(len(resp.Body)))
	} else {
		writeU32(ret[12:16], 0)
		writeU32(ret[16:20], 0)
	}
}

func writeU32(buf []byte, v uint32) {
	buf[0] = byte(v)
	buf[1] = byte(v >> 8)
	buf[2] = byte(v >> 16)
	buf[3] = byte(v >> 24)
}
