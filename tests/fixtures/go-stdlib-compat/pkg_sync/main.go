package main

import (
	"fmt"
	"sync"
)

func main() {
	// sync.Mutex
	var mu sync.Mutex
	mu.Lock()
	mu.Unlock()
	fmt.Println("mutex: ok")

	// sync.WaitGroup
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		fmt.Println("goroutine ran")
	}()
	wg.Wait()
	fmt.Println("waitgroup: ok")

	// sync.Once
	var once sync.Once
	once.Do(func() {
		fmt.Println("once: executed")
	})
	once.Do(func() {
		fmt.Println("once: should not execute")
	})

	// sync.Map
	var m sync.Map
	m.Store("key", "value")
	val, ok := m.Load("key")
	fmt.Println("map load:", val, ok)
}
