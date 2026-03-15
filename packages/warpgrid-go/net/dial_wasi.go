// WASI-specific convenience functions for DNS-aware dialing.
//
// On WASI targets, DefaultDialer() returns a Dialer wired to the
// WarpGrid DNS shim backend. The package-level Dial() and DialTimeout()
// functions provide a drop-in API that resolves hostnames via the shim
// before connecting.
//
// This file is only compiled when targeting WASI (wasip1 or wasip2).

//go:build wasip1 || wasip2

package net

import (
	"net"
	"time"

	"github.com/anthropics/warpgrid/packages/warpgrid-go/dns"
)

// DefaultDialer returns a Dialer configured with the WASI DNS backend.
// Use this when you need to customise timeouts or other Dialer fields.
func DefaultDialer() *Dialer {
	return NewDialer(dns.DefaultResolver())
}

// Dial connects to the address on the named network, resolving
// hostnames via the WarpGrid DNS shim. IP literals bypass DNS.
//
// This is the package-level convenience wrapper around DefaultDialer().Dial.
func Dial(network, address string) (net.Conn, error) {
	return DefaultDialer().Dial(network, address)
}

// DialTimeout is like Dial but with a per-address connection timeout.
func DialTimeout(network, address string, timeout time.Duration) (net.Conn, error) {
	d := DefaultDialer()
	d.ConnectTimeout = timeout
	return d.Dial(network, address)
}
