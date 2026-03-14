package main

import (
	"fmt"
	"strconv"
)

func main() {
	// Itoa: int to string
	s := strconv.Itoa(42)
	fmt.Println(s)

	// Atoi: string to int
	n, err := strconv.Atoi("123")
	if err != nil {
		fmt.Println("Atoi error:", err)
	}
	fmt.Println(n)

	// FormatFloat
	f := strconv.FormatFloat(3.14159, 'f', 4, 64)
	fmt.Println(f)

	// ParseBool
	b, err := strconv.ParseBool("true")
	if err != nil {
		fmt.Println("ParseBool error:", err)
	}
	fmt.Println(b)
}
