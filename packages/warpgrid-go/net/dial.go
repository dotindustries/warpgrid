// Package net provides a DNS-aware Dial function for WarpGrid WASI modules.
//
// The Dialer resolves hostnames through the WarpGrid DNS shim before
// attempting TCP/UDP connections. IP literals bypass DNS entirely.
// When multiple addresses are returned, each is tried in order until
// one succeeds (basic failover). DNS failures are wrapped as *net.OpError
// for compatibility with standard Go error handling patterns.
//
// This package is part of the WarpGrid Go overlay (Domain 3, US-304).
package net

import (
	"fmt"
	"net"
	"time"

	"github.com/anthropics/warpgrid/packages/warpgrid-go/dns"
)

// Dialer resolves hostnames via a dns.Resolver and dials TCP/UDP
// connections with ordered failover across multiple A records.
type Dialer struct {
	resolver *dns.Resolver

	// ConnectTimeout is the per-address connection timeout.
	// When zero, net.Dialer uses its default (no timeout).
	ConnectTimeout time.Duration
}

// NewDialer creates a Dialer that resolves hostnames via the given resolver.
func NewDialer(resolver *dns.Resolver) *Dialer {
	return &Dialer{resolver: resolver}
}

// Dial connects to the address on the named network.
//
// If the host component is an IP literal, it is used directly without
// DNS resolution. Otherwise, the hostname is resolved via the WarpGrid
// DNS shim and each returned address is tried in order. The first
// successful connection is returned. If all addresses fail, the last
// error is returned wrapped as *net.OpError.
//
// Supported networks: "tcp", "tcp4", "tcp6", "udp", "udp4", "udp6".
func (d *Dialer) Dial(network, address string) (net.Conn, error) {
	host, port, err := net.SplitHostPort(address)
	if err != nil {
		return nil, &net.OpError{
			Op:  "dial",
			Net: network,
			Err: fmt.Errorf("invalid address %q: %w", address, err),
		}
	}

	// IP literal: dial directly, no DNS needed
	if dns.IsIPLiteral(host) {
		return d.dialDirect(network, address)
	}

	// Resolve hostname via WarpGrid DNS shim
	ips, err := d.resolver.Resolve(host)
	if err != nil {
		return nil, &net.OpError{
			Op:  "dial",
			Net: network,
			Err: &net.DNSError{
				Err:        err.Error(),
				Name:       host,
				IsNotFound: true,
			},
		}
	}

	if len(ips) == 0 {
		return nil, &net.OpError{
			Op:  "dial",
			Net: network,
			Err: &net.DNSError{
				Err:        "no addresses found",
				Name:       host,
				IsNotFound: true,
			},
		}
	}

	// Try each resolved address in order (failover)
	var lastErr error
	for _, ip := range ips {
		addr := net.JoinHostPort(ip.String(), port)
		conn, err := d.dialDirect(network, addr)
		if err == nil {
			return conn, nil
		}
		lastErr = err
	}

	return nil, &net.OpError{
		Op:  "dial",
		Net: network,
		Err: fmt.Errorf("all %d addresses failed for %s: %w", len(ips), host, lastErr),
	}
}

// dialDirect connects to an address without DNS resolution.
func (d *Dialer) dialDirect(network, address string) (net.Conn, error) {
	dialer := &net.Dialer{}
	if d.ConnectTimeout > 0 {
		dialer.Timeout = d.ConnectTimeout
	}
	return dialer.Dial(network, address)
}
