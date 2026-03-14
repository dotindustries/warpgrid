package main

import (
	"crypto/sha256"
	"fmt"
)

func main() {
	msg := []byte("hello wasm")

	// Sum256: one-shot hash
	hash := sha256.Sum256(msg)
	fmt.Printf("sha256: %x\n", hash)

	// New + Write + Sum: streaming hash
	h := sha256.New()
	h.Write([]byte("hello "))
	h.Write([]byte("wasm"))
	result := h.Sum(nil)
	fmt.Printf("streaming: %x\n", result)

	// Verify both produce the same result
	fmt.Println("match:", fmt.Sprintf("%x", hash[:]) == fmt.Sprintf("%x", result))
}
