package main

import (
	"fmt"
	"regexp"
)

func main() {
	// regexp.MustCompile
	re := regexp.MustCompile(`\d+`)

	// FindString
	match := re.FindString("abc123def")
	fmt.Println("found:", match)

	// ReplaceAllString
	replaced := re.ReplaceAllString("abc123def456", "NUM")
	fmt.Println("replaced:", replaced)

	// MatchString
	matched := re.MatchString("no digits here")
	fmt.Println("has digits:", matched)

	// FindAllString
	all := re.FindAllString("a1b2c3", -1)
	fmt.Println("all matches:", all)
}
