// go-tcp-echo is an acceptance test fixture for US-303 (net.Dial TCP via wasi-sockets).
//
// It exercises the full net.Dial("tcp", addr) path:
//   1. Connects to a TCP echo server (address from env or default)
//   2. Sends a known payload
//   3. Reads back the echo response
//   4. Verifies the response matches the sent data
//   5. Tests error handling for connection to unreachable address
//
// When compiled with TinyGo for wasip2, this validates that the wasip2Netdev
// implementation correctly wires wasi:sockets/tcp to Go's net package.
package main

import (
	"fmt"
	"io"
	"net"
	"os"
	"time"
)

func main() {
	addr := os.Getenv("ECHO_SERVER_ADDR")
	if addr == "" {
		addr = "127.0.0.1:7"
	}

	passed := 0
	failed := 0

	// Test 1: TCP dial + send + receive echo
	if err := testEcho(addr); err != nil {
		fmt.Printf("FAIL test_echo: %v\n", err)
		failed++
	} else {
		fmt.Println("PASS test_echo")
		passed++
	}

	// Test 2: Connection to unreachable address returns error
	if err := testDialError(); err != nil {
		fmt.Printf("FAIL test_dial_error: %v\n", err)
		failed++
	} else {
		fmt.Println("PASS test_dial_error")
		passed++
	}

	// Test 3: Multiple sequential connections
	if err := testMultipleConnections(addr); err != nil {
		fmt.Printf("FAIL test_multiple_connections: %v\n", err)
		failed++
	} else {
		fmt.Println("PASS test_multiple_connections")
		passed++
	}

	fmt.Printf("\nResults: %d passed, %d failed\n", passed, failed)
	if failed > 0 {
		os.Exit(1)
	}
}

// testEcho dials the echo server, sends a payload, and verifies the response.
func testEcho(addr string) error {
	conn, err := net.Dial("tcp", addr)
	if err != nil {
		return fmt.Errorf("net.Dial(%q): %w", addr, err)
	}
	defer conn.Close()

	// Set a deadline so we don't hang forever
	if err := conn.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
		return fmt.Errorf("SetDeadline: %w", err)
	}

	payload := []byte("Hello from WarpGrid!")
	n, err := conn.Write(payload)
	if err != nil {
		return fmt.Errorf("Write: %w", err)
	}
	if n != len(payload) {
		return fmt.Errorf("Write: wrote %d bytes, want %d", n, len(payload))
	}

	buf := make([]byte, len(payload))
	n, err = io.ReadFull(conn, buf)
	if err != nil {
		return fmt.Errorf("ReadFull: %w (read %d bytes)", err, n)
	}

	if string(buf) != string(payload) {
		return fmt.Errorf("echo mismatch: got %q, want %q", buf, payload)
	}

	return nil
}

// testDialError verifies that connecting to an unreachable address returns
// an appropriate error (not a panic or hang).
func testDialError() error {
	// RFC 5737 TEST-NET-1: 192.0.2.0/24 is reserved for documentation, should be unreachable.
	// Use port 1 which is unlikely to have a listener.
	conn, err := net.DialTimeout("tcp", "192.0.2.1:1", 2*time.Second)
	if err == nil {
		conn.Close()
		return fmt.Errorf("expected error dialing unreachable address, got nil")
	}

	// Verify we got a net.OpError (or at least some error)
	if _, ok := err.(*net.OpError); !ok {
		// On some platforms the error type may differ; just ensure it's non-nil
		fmt.Printf("  note: error type is %T (not *net.OpError): %v\n", err, err)
	}

	return nil
}

// testMultipleConnections verifies that the fd table handles multiple
// sequential connections correctly (allocate, use, close, repeat).
func testMultipleConnections(addr string) error {
	for i := 0; i < 3; i++ {
		conn, err := net.Dial("tcp", addr)
		if err != nil {
			return fmt.Errorf("connection %d: net.Dial: %w", i, err)
		}

		if err := conn.SetDeadline(time.Now().Add(5 * time.Second)); err != nil {
			conn.Close()
			return fmt.Errorf("connection %d: SetDeadline: %w", i, err)
		}

		msg := fmt.Sprintf("msg-%d", i)
		if _, err := conn.Write([]byte(msg)); err != nil {
			conn.Close()
			return fmt.Errorf("connection %d: Write: %w", i, err)
		}

		buf := make([]byte, len(msg))
		if _, err := io.ReadFull(conn, buf); err != nil {
			conn.Close()
			return fmt.Errorf("connection %d: ReadFull: %w", i, err)
		}

		if string(buf) != msg {
			conn.Close()
			return fmt.Errorf("connection %d: echo mismatch: got %q, want %q", i, buf, msg)
		}

		conn.Close()
	}
	return nil
}
