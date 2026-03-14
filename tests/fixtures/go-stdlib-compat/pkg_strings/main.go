package main

import (
	"fmt"
	"strings"
)

func main() {
	// Contains
	fmt.Println(strings.Contains("hello world", "world"))

	// Split
	parts := strings.Split("a,b,c", ",")
	fmt.Println(len(parts))

	// Join
	joined := strings.Join(parts, "-")
	fmt.Println(joined)

	// Replace
	replaced := strings.Replace("aaa", "a", "b", 2)
	fmt.Println(replaced)

	// TrimSpace
	trimmed := strings.TrimSpace("  hello  ")
	fmt.Println(trimmed)
}
