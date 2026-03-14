// DNSError polyfill for environments where net.DNSError is unavailable.
//
// TinyGo's net package does not include net.DNSError, so we define a
// compatible type here. On standard Go, we use net.DNSError directly
// (see dnserror_std.go). This file is only compiled under TinyGo.

//go:build tinygo

package net

// DNSError represents a DNS lookup failure, compatible with Go's
// net.DNSError interface. This polyfill is used by TinyGo builds
// where net.DNSError is not available.
type DNSError struct {
	Err        string
	Name       string
	IsNotFound bool
}

func (e *DNSError) Error() string {
	s := "lookup " + e.Name
	if e.Err != "" {
		s += ": " + e.Err
	}
	return s
}

func (e *DNSError) Timeout() bool   { return false }
func (e *DNSError) Temporary() bool { return false }
