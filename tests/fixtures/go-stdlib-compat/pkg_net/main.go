package main

import (
	"fmt"
	"net"
)

func main() {
	// net.ParseCIDR
	ip, ipNet, err := net.ParseCIDR("192.168.1.0/24")
	if err != nil {
		fmt.Println("ParseCIDR error:", err)
	} else {
		fmt.Println("IP:", ip)
		fmt.Println("Network:", ipNet)
	}

	// net.ParseIP
	parsed := net.ParseIP("127.0.0.1")
	fmt.Println("parsed IP:", parsed)

	// net.JoinHostPort / net.SplitHostPort
	hostport := net.JoinHostPort("localhost", "8080")
	fmt.Println("joined:", hostport)
	host, port, err := net.SplitHostPort(hostport)
	if err != nil {
		fmt.Println("SplitHostPort error:", err)
	} else {
		fmt.Println("host:", host, "port:", port)
	}

	// net.Dial — known to have issues in TinyGo wasip2
	conn, err := net.Dial("tcp", "127.0.0.1:0")
	if err != nil {
		fmt.Println("Dial error (expected in wasm):", err)
	} else {
		conn.Close()
		fmt.Println("Dial: connected")
	}
}
