package main

import (
	"fmt"
	"time"
)

func main() {
	// time.Now
	now := time.Now()
	fmt.Println("now:", now.Format(time.RFC3339))

	// time.Parse
	t, err := time.Parse(time.RFC3339, "2026-01-01T00:00:00Z")
	if err != nil {
		fmt.Println("Parse error:", err)
	} else {
		fmt.Println("parsed:", t.Year())
	}

	// time.Duration
	d := 5 * time.Second
	fmt.Println("duration:", d)

	// time.NewTicker (brief)
	ticker := time.NewTicker(100 * time.Millisecond)
	<-ticker.C
	ticker.Stop()
	fmt.Println("ticker: ok")
}
