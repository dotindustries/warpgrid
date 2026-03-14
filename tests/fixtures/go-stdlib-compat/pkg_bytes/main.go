package main

import (
	"bytes"
	"fmt"
	"io"
)

func main() {
	// bytes.Buffer
	var buf bytes.Buffer
	buf.WriteString("hello ")
	buf.WriteString("wasm")
	fmt.Println("buffer:", buf.String())

	// bytes.Contains
	fmt.Println("contains:", bytes.Contains([]byte("hello world"), []byte("world")))

	// bytes.Join
	joined := bytes.Join([][]byte{[]byte("a"), []byte("b"), []byte("c")}, []byte(","))
	fmt.Println("joined:", string(joined))

	// bytes.NewReader
	reader := bytes.NewReader([]byte("test data"))
	data, err := io.ReadAll(reader)
	if err != nil {
		fmt.Println("read error:", err)
		return
	}
	fmt.Println("read:", string(data))
}
