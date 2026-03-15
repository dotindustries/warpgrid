// Non-WASI fallback convenience functions for DNS-aware dialing.
//
// On standard Go (non-WASI), there is no WarpGrid DNS shim backend.
// These functions fall through to the standard library's net.Dial so
// that code importing this package compiles and works in native
// development and testing environments.

//go:build !wasip1 && !wasip2

package net

import (
	"net"
	"time"
)

// Dial connects to the address on the named network.
//
// On non-WASI targets this delegates directly to net.Dial from the
// standard library since no WarpGrid DNS shim is available.
func Dial(network, address string) (net.Conn, error) {
	return net.Dial(network, address)
}

// DialTimeout is like Dial but with a connection timeout.
//
// On non-WASI targets this delegates to net.DialTimeout.
func DialTimeout(network, address string, timeout time.Duration) (net.Conn, error) {
	return net.DialTimeout(network, address, timeout)
}
