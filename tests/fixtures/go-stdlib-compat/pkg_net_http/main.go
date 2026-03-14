package main

import (
	"fmt"
	"net/http"
	"strings"
)

func main() {
	// http.NewRequest
	req, err := http.NewRequest("GET", "http://example.com/api", nil)
	if err != nil {
		fmt.Println("NewRequest error:", err)
		return
	}
	fmt.Println("method:", req.Method)
	fmt.Println("url:", req.URL.String())

	// http.Header
	h := make(http.Header)
	h.Set("Content-Type", "application/json")
	h.Add("Accept", "text/html")
	fmt.Println("content-type:", h.Get("Content-Type"))

	// http.StatusText
	fmt.Println("200:", http.StatusText(200))
	fmt.Println("404:", http.StatusText(404))

	// http.NewRequest with body
	body := strings.NewReader(`{"key":"value"}`)
	req2, err := http.NewRequest("POST", "http://example.com/data", body)
	if err != nil {
		fmt.Println("NewRequest POST error:", err)
		return
	}
	fmt.Println("POST method:", req2.Method)
}
