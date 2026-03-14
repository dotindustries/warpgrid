package main

import (
	"fmt"
	"os"
)

func main() {
	// os.Getenv
	path := os.Getenv("PATH")
	fmt.Println("PATH length:", len(path))

	// os.Stdout.Write
	os.Stdout.Write([]byte("stdout write ok\n"))

	// os.Args
	fmt.Println("args count:", len(os.Args))

	// os.TempDir
	tmp := os.TempDir()
	fmt.Println("temp dir:", tmp)
}
