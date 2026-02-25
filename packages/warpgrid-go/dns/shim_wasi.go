// WASI-specific DNS resolver backend using WarpGrid host shim.
//
// This file is only compiled when targeting WASI (wasip1 or wasip2).
// It calls the warpgrid:shim/dns.resolve-address host function
// through a low-level ABI compatible with the wasi-libc DNS shim.
//
// ABI contract (matching libc-patches/0001-dns-getaddrinfo):
//   Input: hostname (ptr, len), family (0 = any), out_buf (ptr), out_buf_cap
//   Output: count of records written, each record = 17 bytes:
//     byte 0: family marker (4 = IPv4, 6 = IPv6)
//     bytes 1-4: IPv4 address (when family=4)
//     bytes 1-16: IPv6 address (when family=6)

//go:build wasip1 || wasip2

package dns

import (
	"fmt"
	"net"
	"unsafe"
)

// warpgridDnsResolve is the host-imported DNS resolution function.
// It matches the ABI of __warpgrid_dns_resolve from the libc patches.
//
//go:wasmimport warpgrid_shim dns_resolve
func warpgridDnsResolve(
	hostnamePtr unsafe.Pointer,
	hostnameLen uint32,
	family uint32,
	outBufPtr unsafe.Pointer,
	outBufCap uint32,
) uint32

const (
	familyAny  = 0
	familyIPv4 = 4
	familyIPv6 = 6
	recordSize = 17 // 1 byte family + 16 bytes address
	maxRecords = 32
)

// WasiBackend implements ResolverBackend by calling the WarpGrid DNS
// host shim through the //go:wasmimport directive.
type WasiBackend struct{}

// Resolve calls warpgrid:shim/dns.resolve-address for the given hostname.
func (WasiBackend) Resolve(hostname string) ([]net.IP, error) {
	if hostname == "" {
		return nil, fmt.Errorf("dns: empty hostname")
	}

	buf := make([]byte, maxRecords*recordSize)
	hostnameBytes := []byte(hostname)

	count := warpgridDnsResolve(
		unsafe.Pointer(&hostnameBytes[0]),
		uint32(len(hostnameBytes)),
		familyAny,
		unsafe.Pointer(&buf[0]),
		uint32(len(buf)),
	)

	if count == 0 {
		return nil, fmt.Errorf("dns: host not found: %s", hostname)
	}

	// Clamp to buffer capacity to prevent out-of-bounds access
	// if the host returns a count larger than our buffer can hold.
	bufCap := uint32(len(buf) / recordSize)
	if count > bufCap {
		count = bufCap
	}

	ips := make([]net.IP, 0, count)
	for i := uint32(0); i < count; i++ {
		offset := i * recordSize
		family := buf[offset]
		addrBytes := buf[offset+1 : offset+recordSize]

		switch family {
		case familyIPv4:
			ip := make(net.IP, 4)
			copy(ip, addrBytes[:4])
			ips = append(ips, ip)
		case familyIPv6:
			ip := make(net.IP, 16)
			copy(ip, addrBytes[:16])
			ips = append(ips, ip)
		}
	}

	if len(ips) == 0 {
		return nil, fmt.Errorf("dns: host not found: %s", hostname)
	}

	return ips, nil
}

// DefaultResolver returns a Resolver configured with the WASI backend.
// Use this in WASI modules to get DNS resolution via the WarpGrid shim.
func DefaultResolver() *Resolver {
	return NewResolver(WasiBackend{})
}
