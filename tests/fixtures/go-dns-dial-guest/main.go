// Package main is a WASI guest fixture that exercises DNS resolution
// through the WarpGrid DNS shim (warpgrid_shim dns_resolve).
//
// Each exported function tests a specific DNS resolution scenario via
// dns.DefaultResolver(). The host integration test registers a service
// registry mapping test hostnames to known IPs, then calls these exports
// and validates the results.
//
// Build: tinygo build -target=wasi -buildmode=c-shared -o go-dns-dial-guest.wasm .
//
// Reactor mode (-buildmode=c-shared) is required so that //go:wasmexport
// functions work after _initialize.
package main

import (
	"errors"
	"fmt"
	"net"
	"strings"
	"unsafe"

	wgdns "github.com/anthropics/warpgrid/packages/warpgrid-go/dns"
	wgnet "github.com/anthropics/warpgrid/packages/warpgrid-go/net"
)

func main() {}

// ── Exported test functions ─────────────────────────────────────────
//
// Each function returns a packed uint64: high 32 bits = pointer to
// result string, low 32 bits = length. The host reads the string from
// linear memory. Format: "OK:<data>" on success, "ERR:<message>" on failure.

// testResolveRegistry resolves a hostname expected to be in the service
// registry and returns the first resolved IP address.
//
//go:wasmexport test-resolve-registry
func testResolveRegistry() uint64 {
	resolver := wgdns.DefaultResolver()
	ips, err := resolver.Resolve("echo-server.test.warp.local")
	if err != nil {
		return writeResult(fmt.Sprintf("ERR:resolve failed: %v", err))
	}
	if len(ips) == 0 {
		return writeResult("ERR:no addresses returned")
	}
	return writeResult(fmt.Sprintf("OK:%s", ips[0].String()))
}

// testResolveMultiple resolves a hostname with multiple A records and
// returns all addresses comma-separated.
//
//go:wasmexport test-resolve-multiple
func testResolveMultiple() uint64 {
	resolver := wgdns.DefaultResolver()
	ips, err := resolver.Resolve("multi.test.warp.local")
	if err != nil {
		return writeResult(fmt.Sprintf("ERR:resolve failed: %v", err))
	}
	if len(ips) == 0 {
		return writeResult("ERR:no addresses returned")
	}
	parts := make([]string, len(ips))
	for i, ip := range ips {
		parts[i] = ip.String()
	}
	return writeResult(fmt.Sprintf("OK:%s", strings.Join(parts, ",")))
}

// testResolveNonexistent attempts to resolve a hostname that does not
// exist and returns the error message.
//
//go:wasmexport test-resolve-nonexistent
func testResolveNonexistent() uint64 {
	resolver := wgdns.DefaultResolver()
	_, err := resolver.Resolve("nonexistent.invalid")
	if err == nil {
		return writeResult("ERR:expected error for nonexistent hostname, got nil")
	}
	return writeResult(fmt.Sprintf("OK:%s", err.Error()))
}

// testResolveIPLiteral verifies that IP literals bypass DNS resolution
// and are returned directly.
//
//go:wasmexport test-resolve-ip-literal
func testResolveIPLiteral() uint64 {
	resolver := wgdns.DefaultResolver()
	ips, err := resolver.Resolve("192.168.1.1")
	if err != nil {
		return writeResult(fmt.Sprintf("ERR:resolve failed: %v", err))
	}
	if len(ips) != 1 {
		return writeResult(fmt.Sprintf("ERR:expected 1 address, got %d", len(ips)))
	}
	if ips[0].String() != "192.168.1.1" {
		return writeResult(fmt.Sprintf("ERR:expected 192.168.1.1, got %s", ips[0].String()))
	}
	return writeResult("OK:192.168.1.1")
}

// testDialerDNSErrorWrapping exercises the Dialer with a hostname that
// fails DNS resolution and verifies proper error wrapping.
//
//go:wasmexport test-dialer-dns-error
func testDialerDNSErrorWrapping() uint64 {
	dialer := wgnet.DefaultDialer()
	_, err := dialer.Dial("tcp", "nonexistent.invalid:5432")
	if err == nil {
		return writeResult("ERR:expected error, got nil")
	}

	// Verify *net.OpError wrapping
	var opErr *net.OpError
	if !errors.As(err, &opErr) {
		return writeResult(fmt.Sprintf("ERR:expected *net.OpError, got %T: %v", err, err))
	}
	if opErr.Op != "dial" {
		return writeResult(fmt.Sprintf("ERR:expected Op=dial, got %s", opErr.Op))
	}

	// Verify inner *wgnet.DNSError
	var dnsErr *wgnet.DNSError
	if !errors.As(opErr.Err, &dnsErr) {
		return writeResult(fmt.Sprintf("ERR:expected inner *DNSError, got %T: %v", opErr.Err, opErr.Err))
	}
	if dnsErr.Name != "nonexistent.invalid" {
		return writeResult(fmt.Sprintf("ERR:DNSError.Name=%q, want nonexistent.invalid", dnsErr.Name))
	}

	return writeResult("OK:correctly wrapped as *net.OpError{*DNSError}")
}

// ── Helper: pack result string into uint64 (ptr << 32 | len) ───────

var resultBuf []byte

func writeResult(s string) uint64 {
	resultBuf = []byte(s)
	ptr := uint64(uintptr(unsafe.Pointer(&resultBuf[0])))
	length := uint64(len(resultBuf))
	return (ptr << 32) | length
}
