package main

import (
	"bytes"
	"encoding/json"
	"fmt"
)

type Person struct {
	Name string `json:"name"`
	Age  int    `json:"age"`
}

func main() {
	// Marshal: struct to JSON
	p := Person{Name: "Alice", Age: 30}
	data, err := json.Marshal(p)
	if err != nil {
		fmt.Println("Marshal error:", err)
		return
	}
	fmt.Println(string(data))

	// Unmarshal: JSON to struct
	var p2 Person
	err = json.Unmarshal(data, &p2)
	if err != nil {
		fmt.Println("Unmarshal error:", err)
		return
	}
	fmt.Println(p2.Name, p2.Age)

	// NewEncoder: write JSON to a buffer
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	err = enc.Encode(p)
	if err != nil {
		fmt.Println("Encode error:", err)
		return
	}
	fmt.Println(buf.String())
}
