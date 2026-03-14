package main

import (
	"bytes"
	"log"
)

func main() {
	// log.Println
	log.Println("log message")

	// log.Printf
	log.Printf("formatted: %d", 42)

	// log.New with custom writer
	var buf bytes.Buffer
	logger := log.New(&buf, "PREFIX: ", 0)
	logger.Println("custom logger")
	log.Println("custom output:", buf.String())

	// log.SetPrefix
	log.SetPrefix("[WASM] ")
	log.Println("prefixed message")
}
