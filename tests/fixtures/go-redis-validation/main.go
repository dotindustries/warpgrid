// Package main validates go-redis/redis compilation and runtime behavior with TinyGo wasip2.
//
// US-308: Database driver compatibility — MySQL and Redis
//
// This program imports github.com/redis/go-redis/v9 and exercises:
//   - redis.NewClient() with Options
//   - PING command
//   - SET/GET key-value cycle
//
// When compiled with TinyGo wasip2, any unsupported stdlib dependencies
// surface as compilation errors. These are documented in compat-db/tinygo-drivers.json.
package main

import (
	"context"
	"fmt"
	"os"

	"github.com/redis/go-redis/v9"
)

func main() {
	addr := os.Getenv("REDIS_ADDR")
	if addr == "" {
		addr = "localhost:6379"
	}

	ctx := context.Background()

	client := connectRedis(addr)
	defer client.Close()

	if err := runPing(ctx, client); err != nil {
		fmt.Fprintf(os.Stderr, "PING failed: %v\n", err)
		os.Exit(1)
	}

	if err := runSetGet(ctx, client); err != nil {
		fmt.Fprintf(os.Stderr, "SET/GET failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("redis validation: all operations succeeded")
}

// connectRedis creates a new Redis client with the given address.
func connectRedis(addr string) *redis.Client {
	return redis.NewClient(&redis.Options{
		Addr:     addr,
		Password: "",
		DB:       0,
	})
}

// runPing sends a PING command and verifies the response.
func runPing(ctx context.Context, client *redis.Client) error {
	result, err := client.Ping(ctx).Result()
	if err != nil {
		return fmt.Errorf("PING: %w", err)
	}
	if result != "PONG" {
		return fmt.Errorf("PING returned %q, expected %q", result, "PONG")
	}
	fmt.Println("PING: OK")
	return nil
}

// runSetGet performs a SET followed by a GET and verifies the value.
func runSetGet(ctx context.Context, client *redis.Client) error {
	key := "warpgrid:redis-validation:test-key"
	value := "redis-test-value"

	err := client.Set(ctx, key, value, 0).Err()
	if err != nil {
		return fmt.Errorf("SET: %w", err)
	}
	fmt.Println("SET: OK")

	got, err := client.Get(ctx, key).Result()
	if err != nil {
		return fmt.Errorf("GET: %w", err)
	}
	if got != value {
		return fmt.Errorf("GET returned %q, expected %q", got, value)
	}
	fmt.Println("GET: OK")

	// Cleanup
	client.Del(ctx, key)

	return nil
}

// getRedisClientInfo validates that core go-redis types are importable.
// This function exists primarily to force the compiler to resolve type imports.
func getRedisClientInfo() map[string]string {
	return map[string]string{
		"client_type":  fmt.Sprintf("%T", (*redis.Client)(nil)),
		"options_type": fmt.Sprintf("%T", (*redis.Options)(nil)),
		"cmd_type":     fmt.Sprintf("%T", (*redis.Cmd)(nil)),
		"status_type":  fmt.Sprintf("%T", (*redis.StatusCmd)(nil)),
		"string_type":  fmt.Sprintf("%T", (*redis.StringCmd)(nil)),
	}
}
