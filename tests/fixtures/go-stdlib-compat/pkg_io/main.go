package main

import (
	"bytes"
	"fmt"
	"io"
)

func main() {
	// io.Copy
	src := bytes.NewReader([]byte("copy this"))
	var dst bytes.Buffer
	n, err := io.Copy(&dst, src)
	if err != nil {
		fmt.Println("Copy error:", err)
		return
	}
	fmt.Printf("copied %d bytes: %s\n", n, dst.String())

	// io.ReadAll
	reader := bytes.NewReader([]byte("read all"))
	data, err := io.ReadAll(reader)
	if err != nil {
		fmt.Println("ReadAll error:", err)
		return
	}
	fmt.Println("readall:", string(data))

	// io.NopCloser
	rc := io.NopCloser(bytes.NewReader([]byte("nop")))
	d2, _ := io.ReadAll(rc)
	rc.Close()
	fmt.Println("nopCloser:", string(d2))

	// io.Pipe
	pr, pw := io.Pipe()
	go func() {
		pw.Write([]byte("pipe data"))
		pw.Close()
	}()
	pipeData, _ := io.ReadAll(pr)
	fmt.Println("pipe:", string(pipeData))
}
