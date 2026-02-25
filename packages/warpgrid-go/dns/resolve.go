// Package dns provides hostname resolution for WarpGrid WASI modules.
//
// On WASI targets, resolution delegates to the warpgrid:shim/dns
// host function (resolve-address). On standard Go (for testing and
// native development), a pluggable backend allows mock injection.
//
// This package is part of the WarpGrid Go overlay (Domain 3, US-304).
package dns

import (
	"fmt"
	"net"
	"strings"
)

// ResolverBackend abstracts the platform-specific DNS resolution call.
//
// On WASI, this calls warpgrid:shim/dns.resolve-address via
// //go:wasmimport. On standard Go, tests inject a mock implementation.
type ResolverBackend interface {
	Resolve(hostname string) ([]net.IP, error)
}

// Resolver wraps a ResolverBackend with IP literal detection and
// validation logic. When the input is already an IP address, the
// backend is bypassed entirely.
type Resolver struct {
	backend ResolverBackend
}

// NewResolver creates a Resolver with the given backend.
func NewResolver(backend ResolverBackend) *Resolver {
	return &Resolver{backend: backend}
}

// Resolve resolves a hostname to a list of IP addresses.
//
// If hostname is an IP literal (IPv4, IPv6, or bracketed IPv6),
// it is returned directly without calling the backend.
// Otherwise, the backend is consulted for resolution.
func (r *Resolver) Resolve(hostname string) ([]net.IP, error) {
	// Fast path: IP literals bypass DNS entirely
	if IsIPLiteral(hostname) {
		stripped := strings.TrimPrefix(strings.TrimSuffix(hostname, "]"), "[")
		ip := net.ParseIP(stripped)
		if ip == nil {
			return nil, fmt.Errorf("dns: IsIPLiteral matched but ParseIP failed for %q", hostname)
		}
		return []net.IP{ip}, nil
	}

	return r.backend.Resolve(hostname)
}

// IsIPLiteral reports whether s is an IP address literal.
//
// Recognises bare IPv4 ("127.0.0.1"), bare IPv6 ("::1"), and
// bracketed IPv6 ("[::1]") as used in host:port addresses.
func IsIPLiteral(s string) bool {
	if s == "" {
		return false
	}

	// Handle bracketed IPv6 (e.g. "[::1]")
	if strings.HasPrefix(s, "[") && strings.HasSuffix(s, "]") {
		inner := s[1 : len(s)-1]
		return net.ParseIP(inner) != nil
	}

	return net.ParseIP(s) != nil
}
