package http

// bufferResponseWriter captures the response in memory for later
// serialization to the WIT wire format. Implements ResponseWriter.
type bufferResponseWriter struct {
	header      Header
	body        []byte
	statusCode  int
	wroteHeader bool
}

func newBufferResponseWriter() *bufferResponseWriter {
	return &bufferResponseWriter{
		header:     make(Header),
		statusCode: StatusOK,
	}
}

func (w *bufferResponseWriter) Header() Header {
	return w.header
}

func (w *bufferResponseWriter) Write(data []byte) (int, error) {
	if !w.wroteHeader {
		w.wroteHeader = true
	}
	w.body = append(w.body, data...)
	return len(data), nil
}

func (w *bufferResponseWriter) WriteHeader(statusCode int) {
	if w.wroteHeader {
		return
	}
	w.wroteHeader = true
	w.statusCode = statusCode
}

// StatusCode returns the captured status code.
func (w *bufferResponseWriter) StatusCode() int {
	return w.statusCode
}

// Body returns the captured response body bytes.
func (w *bufferResponseWriter) Body() []byte {
	return w.body
}

// NewTestResponseWriter creates a ResponseWriter that captures the
// response for test assertions. The returned value also provides
// StatusCode() and Body() accessors.
func NewTestResponseWriter() *bufferResponseWriter {
	return newBufferResponseWriter()
}
