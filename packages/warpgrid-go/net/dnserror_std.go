// DNSError alias for standard Go (non-TinyGo) builds.
//
// On standard Go, net.DNSError is available directly. We re-export it
// so that dial.go can use DNSError without conditional imports.

//go:build !tinygo

package net

import "net"

// DNSError is an alias for net.DNSError on standard Go builds.
type DNSError = net.DNSError
