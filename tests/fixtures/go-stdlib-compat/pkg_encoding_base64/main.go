package main

import (
	"encoding/base64"
	"fmt"
)

func main() {
	msg := "Hello, WebAssembly!"

	// EncodeToString
	encoded := base64.StdEncoding.EncodeToString([]byte(msg))
	fmt.Println(encoded)

	// DecodeString
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		fmt.Println("Decode error:", err)
		return
	}
	fmt.Println(string(decoded))

	// URLEncoding variant
	urlEncoded := base64.URLEncoding.EncodeToString([]byte(msg))
	fmt.Println(urlEncoded)
}
