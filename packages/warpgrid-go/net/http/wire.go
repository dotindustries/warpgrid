package http

import "encoding/binary"

// WIT type equivalents matching crates/warpgrid-host/wit/http-types.wit.

// WitHttpHeader represents an HTTP header name-value pair.
type WitHttpHeader struct {
	Name  string
	Value string
}

// WitHttpRequest mirrors the WIT http-request record.
type WitHttpRequest struct {
	Method  string
	URI     string
	Headers []WitHttpHeader
	Body    []byte
}

// WitHttpResponse mirrors the WIT http-response record.
type WitHttpResponse struct {
	Status  uint16
	Headers []WitHttpHeader
	Body    []byte
}

// Wire format for serialization between host and guest.
//
// Request format (little-endian):
//   u32: method_len, bytes: method
//   u32: uri_len,    bytes: uri
//   u32: header_count
//     for each: u32: name_len, bytes: name, u32: value_len, bytes: value
//   u32: body_len,   bytes: body
//
// Response format (little-endian):
//   u16: status
//   u32: header_count
//     for each: u32: name_len, bytes: name, u32: value_len, bytes: value
//   u32: body_len,   bytes: body

// MarshalRequest serializes a WitHttpRequest to the wire format.
func MarshalRequest(req WitHttpRequest) []byte {
	size := 4 + len(req.Method) + 4 + len(req.URI) + 4 + 4 + len(req.Body)
	for _, h := range req.Headers {
		size += 4 + len(h.Name) + 4 + len(h.Value)
	}

	buf := make([]byte, 0, size)
	buf = appendString(buf, req.Method)
	buf = appendString(buf, req.URI)
	buf = appendU32(buf, uint32(len(req.Headers)))
	for _, h := range req.Headers {
		buf = appendString(buf, h.Name)
		buf = appendString(buf, h.Value)
	}
	buf = appendBytes(buf, req.Body)
	return buf
}

// UnmarshalRequest deserializes a WitHttpRequest from the wire format.
func UnmarshalRequest(data []byte) WitHttpRequest {
	offset := 0
	var req WitHttpRequest

	req.Method, offset = readString(data, offset)
	req.URI, offset = readString(data, offset)

	headerCount, off := readU32(data, offset)
	offset = off
	req.Headers = make([]WitHttpHeader, headerCount)
	for i := uint32(0); i < headerCount; i++ {
		req.Headers[i].Name, offset = readString(data, offset)
		req.Headers[i].Value, offset = readString(data, offset)
	}

	req.Body, offset = readBytes(data, offset)
	return req
}

// MarshalResponse serializes a WitHttpResponse to the wire format.
func MarshalResponse(resp WitHttpResponse) []byte {
	size := 2 + 4 + 4 + len(resp.Body)
	for _, h := range resp.Headers {
		size += 4 + len(h.Name) + 4 + len(h.Value)
	}

	buf := make([]byte, 0, size)
	buf = appendU16(buf, resp.Status)
	buf = appendU32(buf, uint32(len(resp.Headers)))
	for _, h := range resp.Headers {
		buf = appendString(buf, h.Name)
		buf = appendString(buf, h.Value)
	}
	buf = appendBytes(buf, resp.Body)
	return buf
}

// UnmarshalResponse deserializes a WitHttpResponse from the wire format.
func UnmarshalResponse(data []byte) WitHttpResponse {
	offset := 0
	var resp WitHttpResponse

	status, off := readU16(data, offset)
	resp.Status = status
	offset = off

	headerCount, off := readU32(data, offset)
	offset = off
	resp.Headers = make([]WitHttpHeader, headerCount)
	for i := uint32(0); i < headerCount; i++ {
		resp.Headers[i].Name, offset = readString(data, offset)
		resp.Headers[i].Value, offset = readString(data, offset)
	}

	resp.Body, offset = readBytes(data, offset)
	return resp
}

// ── Encoding helpers ────────────────────────────────────────────────

func appendU16(buf []byte, v uint16) []byte {
	var b [2]byte
	binary.LittleEndian.PutUint16(b[:], v)
	return append(buf, b[:]...)
}

func appendU32(buf []byte, v uint32) []byte {
	var b [4]byte
	binary.LittleEndian.PutUint32(b[:], v)
	return append(buf, b[:]...)
}

func appendString(buf []byte, s string) []byte {
	buf = appendU32(buf, uint32(len(s)))
	return append(buf, s...)
}

func appendBytes(buf []byte, b []byte) []byte {
	buf = appendU32(buf, uint32(len(b)))
	return append(buf, b...)
}

func readU16(data []byte, offset int) (uint16, int) {
	v := binary.LittleEndian.Uint16(data[offset:])
	return v, offset + 2
}

func readU32(data []byte, offset int) (uint32, int) {
	v := binary.LittleEndian.Uint32(data[offset:])
	return v, offset + 4
}

func readString(data []byte, offset int) (string, int) {
	length, off := readU32(data, offset)
	s := string(data[off : off+int(length)])
	return s, off + int(length)
}

func readBytes(data []byte, offset int) ([]byte, int) {
	length, off := readU32(data, offset)
	if length == 0 {
		return nil, off
	}
	b := make([]byte, length)
	copy(b, data[off:off+int(length)])
	return b, off + int(length)
}
