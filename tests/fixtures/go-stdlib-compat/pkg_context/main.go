package main

import (
	"context"
	"fmt"
	"time"
)

func main() {
	// context.Background
	ctx := context.Background()
	fmt.Println("background:", ctx)

	// WithCancel
	ctx2, cancel := context.WithCancel(ctx)
	cancel()
	fmt.Println("cancelled:", ctx2.Err())

	// WithTimeout
	ctx3, cancel3 := context.WithTimeout(ctx, 1*time.Second)
	defer cancel3()
	fmt.Println("timeout deadline set:", ctx3.Err() == nil)

	// WithValue
	type key string
	ctx4 := context.WithValue(ctx, key("user"), "alice")
	val := ctx4.Value(key("user"))
	fmt.Println("value:", val)
}
