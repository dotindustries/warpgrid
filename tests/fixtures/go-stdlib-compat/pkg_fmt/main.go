package main

import (
	"fmt"
	"os"
)

func main() {
	// Sprintf: format a string
	s := fmt.Sprintf("hello %s, count=%d", "world", 42)
	fmt.Println(s)

	// Fprintf: write formatted output to a writer
	fmt.Fprintf(os.Stdout, "pi is approximately %.4f\n", 3.14159)

	// Println: basic output
	fmt.Println("fmt package works")

	// Errorf: create a formatted error
	err := fmt.Errorf("sample error: %d", 404)
	fmt.Println(err.Error())
}
