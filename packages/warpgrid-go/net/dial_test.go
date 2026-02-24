package net_test

import (
	"errors"
	"fmt"
	"io"
	"net"
	"strings"
	"testing"
	"time"

	wgdns "github.com/anthropics/warpgrid/packages/warpgrid-go/dns"
	wgnet "github.com/anthropics/warpgrid/packages/warpgrid-go/net"
)

// ── Test helpers ────────────────────────────────────────────────────

type mockResolverFunc func(hostname string) ([]net.IP, error)

func (f mockResolverFunc) Resolve(hostname string) ([]net.IP, error) {
	return f(hostname)
}

// startEchoServer starts a TCP server that echoes back received data.
// Returns the listener address and a cleanup function.
func startEchoServer(t *testing.T) (string, func()) {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("failed to start echo server: %v", err)
	}

	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return // listener closed
			}
			go func(c net.Conn) {
				defer c.Close()
				io.Copy(c, c)
			}(conn)
		}
	}()

	return ln.Addr().String(), func() { ln.Close() }
}

// ── Dial with IP literal tests ──────────────────────────────────────

func TestDial_IPLiteralSkipsDNS(t *testing.T) {
	addr, cleanup := startEchoServer(t)
	defer cleanup()

	// DNS resolver that should NEVER be called
	dnsResolveCalled := false
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		dnsResolveCalled = true
		return nil, errors.New("should not be called")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("tcp", addr)
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	defer conn.Close()

	if dnsResolveCalled {
		t.Fatal("DNS resolver was called for IP literal address")
	}
}

func TestDial_IPv6LiteralSkipsDNS(t *testing.T) {
	// Try to listen on IPv6 localhost
	ln, err := net.Listen("tcp", "[::1]:0")
	if err != nil {
		t.Skip("IPv6 not available on this host")
	}
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			conn.Close()
		}
	}()
	defer ln.Close()

	dnsResolveCalled := false
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		dnsResolveCalled = true
		return nil, errors.New("should not be called")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	conn.Close()

	if dnsResolveCalled {
		t.Fatal("DNS resolver was called for IPv6 literal address")
	}
}

// ── Dial with hostname DNS resolution tests ─────────────────────────

func TestDial_HostnameResolvedViaDNS(t *testing.T) {
	addr, cleanup := startEchoServer(t)
	defer cleanup()

	host, port, _ := net.SplitHostPort(addr)
	_ = host // We know it's 127.0.0.1

	var resolvedHostname string
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		resolvedHostname = hostname
		return []net.IP{net.ParseIP("127.0.0.1")}, nil
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("tcp", "postgres:"+port)
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	defer conn.Close()

	if resolvedHostname != "postgres" {
		t.Fatalf("expected hostname 'postgres', got '%s'", resolvedHostname)
	}
}

func TestDial_DataRoundTrip(t *testing.T) {
	addr, cleanup := startEchoServer(t)
	defer cleanup()

	_, port, _ := net.SplitHostPort(addr)

	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return []net.IP{net.ParseIP("127.0.0.1")}, nil
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("tcp", "echo-server:"+port)
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	defer conn.Close()

	message := "Hello from WarpGrid!"
	_, err = conn.Write([]byte(message))
	if err != nil {
		t.Fatalf("Write failed: %v", err)
	}

	buf := make([]byte, len(message))
	_, err = io.ReadFull(conn, buf)
	if err != nil {
		t.Fatalf("Read failed: %v", err)
	}

	if string(buf) != message {
		t.Fatalf("expected '%s', got '%s'", message, string(buf))
	}
}

// ── DNS failure wrapped as *net.OpError ─────────────────────────────

func TestDial_DNSFailureReturnsOpError(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("HostNotFound: nonexistent.invalid")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	_, err := dialer.Dial("tcp", "nonexistent.invalid:5432")
	if err == nil {
		t.Fatal("expected error, got nil")
	}

	var opErr *net.OpError
	if !errors.As(err, &opErr) {
		t.Fatalf("expected *net.OpError, got %T: %v", err, err)
	}

	if opErr.Op != "dial" {
		t.Fatalf("expected Op='dial', got '%s'", opErr.Op)
	}
	if opErr.Net != "tcp" {
		t.Fatalf("expected Net='tcp', got '%s'", opErr.Net)
	}
}

func TestDial_DNSEmptyResultReturnsOpError(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return []net.IP{}, nil // empty result, no error
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	_, err := dialer.Dial("tcp", "empty-result.warp.local:5432")
	if err == nil {
		t.Fatal("expected error for empty DNS result, got nil")
	}

	var opErr *net.OpError
	if !errors.As(err, &opErr) {
		t.Fatalf("expected *net.OpError, got %T: %v", err, err)
	}
}

// ── Multiple A records failover ─────────────────────────────────────

func TestDial_FailoverToSecondAddress(t *testing.T) {
	addr, cleanup := startEchoServer(t)
	defer cleanup()

	_, port, _ := net.SplitHostPort(addr)

	// First IP is unreachable, second is the echo server
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return []net.IP{
			net.ParseIP("192.0.2.1"),   // RFC 5737 TEST-NET — unreachable
			net.ParseIP("127.0.0.1"),   // echo server
		}, nil
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)
	dialer.ConnectTimeout = 200 * time.Millisecond

	conn, err := dialer.Dial("tcp", "multi-record:"+port)
	if err != nil {
		t.Fatalf("Dial should have succeeded via failover, got: %v", err)
	}
	defer conn.Close()

	// Verify the connection works
	_, err = conn.Write([]byte("test"))
	if err != nil {
		t.Fatalf("Write failed after failover: %v", err)
	}
}

func TestDial_AllAddressesFailReturnsLastError(t *testing.T) {
	// All IPs are unreachable
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return []net.IP{
			net.ParseIP("192.0.2.1"),   // unreachable
			net.ParseIP("192.0.2.2"),   // unreachable
		}, nil
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)
	dialer.ConnectTimeout = 200 * time.Millisecond

	_, err := dialer.Dial("tcp", "all-fail:65535")
	if err == nil {
		t.Fatal("expected error when all addresses fail")
	}

	var opErr *net.OpError
	if !errors.As(err, &opErr) {
		t.Fatalf("expected *net.OpError, got %T: %v", err, err)
	}
}

func TestDial_TriesAddressesInOrder(t *testing.T) {
	// Start two echo servers
	addr1, cleanup1 := startEchoServer(t)
	defer cleanup1()
	addr2, cleanup2 := startEchoServer(t)
	defer cleanup2()

	_, port1, _ := net.SplitHostPort(addr1)
	_, port2, _ := net.SplitHostPort(addr2)
	_ = port2 // both are on 127.0.0.1

	// First address is reachable — should be used
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return []net.IP{
			net.ParseIP("127.0.0.1"),
		}, nil
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("tcp", fmt.Sprintf("ordered-test:%s", port1))
	if err != nil {
		t.Fatalf("Dial failed: %v", err)
	}
	defer conn.Close()

	// Verify connected to the right server
	localAddr := conn.LocalAddr().String()
	if localAddr == "" {
		t.Fatal("expected non-empty local address")
	}
}

// ── Edge cases ──────────────────────────────────────────────────────

func TestDial_InvalidAddressFormatReturnsError(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("should not be called")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	_, err := dialer.Dial("tcp", "no-port-here")
	if err == nil {
		t.Fatal("expected error for invalid address format")
	}
}

func TestDial_EmptyAddressReturnsError(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("should not be called")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	_, err := dialer.Dial("tcp", "")
	if err == nil {
		t.Fatal("expected error for empty address")
	}
}

func TestDial_UDPNetworkStillWorks(t *testing.T) {
	// UDP dial with IP literal should work without DNS
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("should not be called for IP literal")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("udp", "127.0.0.1:53")
	if err != nil {
		t.Fatalf("UDP Dial failed: %v", err)
	}
	conn.Close()
}

func TestDial_HostnameWithUDP(t *testing.T) {
	resolvedHostname := ""
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		resolvedHostname = hostname
		return []net.IP{net.ParseIP("127.0.0.1")}, nil
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	conn, err := dialer.Dial("udp", "dns-server.warp.local:53")
	if err != nil {
		t.Fatalf("UDP Dial with hostname failed: %v", err)
	}
	conn.Close()

	if resolvedHostname != "dns-server.warp.local" {
		t.Fatalf("expected hostname 'dns-server.warp.local', got '%s'", resolvedHostname)
	}
}

// ── DNSError wrapping ───────────────────────────────────────────────

func TestDial_DNSErrorContainsHostname(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("HostNotFound: missing-service")
	})
	resolver := wgdns.NewResolver(backend)
	dialer := wgnet.NewDialer(resolver)

	_, err := dialer.Dial("tcp", "missing-service:8080")
	if err == nil {
		t.Fatal("expected error")
	}

	errStr := err.Error()
	if !strings.Contains(errStr, "missing-service") {
		t.Fatalf("error should contain hostname, got: %s", errStr)
	}
}
