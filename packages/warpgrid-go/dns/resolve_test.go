package dns_test

import (
	"errors"
	"net"
	"testing"

	"github.com/anthropics/warpgrid/packages/warpgrid-go/dns"
)

// ── MockResolver ────────────────────────────────────────────────────

type mockResolverFunc func(hostname string) ([]net.IP, error)

func (f mockResolverFunc) Resolve(hostname string) ([]net.IP, error) {
	return f(hostname)
}

// ── Resolve tests ───────────────────────────────────────────────────

func TestResolve_ReturnsIPsFromBackend(t *testing.T) {
	expected := []net.IP{net.ParseIP("10.0.0.1")}
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		if hostname == "db.warp.local" {
			return expected, nil
		}
		return nil, errors.New("not found")
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("db.warp.local")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 1 || !ips[0].Equal(expected[0]) {
		t.Fatalf("expected %v, got %v", expected, ips)
	}
}

func TestResolve_ReturnsMultipleIPs(t *testing.T) {
	expected := []net.IP{
		net.ParseIP("10.0.0.1"),
		net.ParseIP("10.0.0.2"),
		net.ParseIP("10.0.0.3"),
	}
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return expected, nil
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("api.warp.local")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 3 {
		t.Fatalf("expected 3 IPs, got %d", len(ips))
	}
}

func TestResolve_ReturnsErrorOnFailure(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("HostNotFound: nonexistent.invalid")
	})

	r := dns.NewResolver(backend)
	_, err := r.Resolve("nonexistent.invalid")
	if err == nil {
		t.Fatal("expected error, got nil")
	}
}

func TestResolve_SupportsIPv6(t *testing.T) {
	expected := []net.IP{net.ParseIP("fd00::1")}
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return expected, nil
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("ipv6-host.warp.local")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 1 {
		t.Fatalf("expected 1 IP, got %d", len(ips))
	}
	if ips[0].To4() != nil {
		t.Fatal("expected IPv6 address, got IPv4")
	}
}

func TestResolve_SupportsMixedIPVersions(t *testing.T) {
	expected := []net.IP{
		net.ParseIP("10.0.0.1"),
		net.ParseIP("fd00::1"),
	}
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return expected, nil
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("dual-stack.warp.local")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if len(ips) != 2 {
		t.Fatalf("expected 2 IPs, got %d", len(ips))
	}
}

func TestResolve_EmptyHostnameReturnsError(t *testing.T) {
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		return nil, errors.New("empty hostname")
	})

	r := dns.NewResolver(backend)
	_, err := r.Resolve("")
	if err == nil {
		t.Fatal("expected error for empty hostname")
	}
}

// ── IsIPLiteral tests ───────────────────────────────────────────────

func TestIsIPLiteral_IPv4(t *testing.T) {
	if !dns.IsIPLiteral("127.0.0.1") {
		t.Fatal("expected 127.0.0.1 to be an IP literal")
	}
}

func TestIsIPLiteral_IPv6(t *testing.T) {
	if !dns.IsIPLiteral("::1") {
		t.Fatal("expected ::1 to be an IP literal")
	}
}

func TestIsIPLiteral_Hostname(t *testing.T) {
	if dns.IsIPLiteral("db.warp.local") {
		t.Fatal("expected db.warp.local to NOT be an IP literal")
	}
}

func TestIsIPLiteral_Empty(t *testing.T) {
	if dns.IsIPLiteral("") {
		t.Fatal("expected empty string to NOT be an IP literal")
	}
}

func TestIsIPLiteral_BracketedIPv6(t *testing.T) {
	// Bracketed IPv6 as it appears in host:port addresses
	if !dns.IsIPLiteral("[::1]") {
		t.Fatal("expected [::1] to be an IP literal")
	}
}

func TestIsIPLiteral_MalformedBracketedInput(t *testing.T) {
	if dns.IsIPLiteral("[not-an-ip]") {
		t.Fatal("expected [not-an-ip] to NOT be an IP literal")
	}
}

// ── Resolve with IP literal input ───────────────────────────────────

func TestResolve_IPLiteralBypassesBackend(t *testing.T) {
	backendCalled := false
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		backendCalled = true
		return nil, errors.New("should not be called")
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("127.0.0.1")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if backendCalled {
		t.Fatal("backend was called for IP literal")
	}
	if len(ips) != 1 || !ips[0].Equal(net.ParseIP("127.0.0.1")) {
		t.Fatalf("expected [127.0.0.1], got %v", ips)
	}
}

func TestResolve_IPv6LiteralBypassesBackend(t *testing.T) {
	backendCalled := false
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		backendCalled = true
		return nil, errors.New("should not be called")
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("::1")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if backendCalled {
		t.Fatal("backend was called for IPv6 literal")
	}
	if len(ips) != 1 {
		t.Fatalf("expected 1 IP, got %d", len(ips))
	}
}

func TestResolve_BracketedIPv6LiteralBypassesBackend(t *testing.T) {
	backendCalled := false
	backend := mockResolverFunc(func(hostname string) ([]net.IP, error) {
		backendCalled = true
		return nil, errors.New("should not be called")
	})

	r := dns.NewResolver(backend)
	ips, err := r.Resolve("[::1]")
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if backendCalled {
		t.Fatal("backend was called for bracketed IPv6 literal")
	}
	if len(ips) != 1 {
		t.Fatalf("expected 1 IP, got %d", len(ips))
	}
}
