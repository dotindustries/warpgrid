// Package main validates that go-redis/redis compiles and functions correctly
// when built with TinyGo targeting wasip2.
//
// This test suite serves as both:
//  1. A standard Go test (go test) — validates logic correctness
//  2. A compilation target for TinyGo wasip2 — validates the Redis client compiles
//
// US-308: Database driver compatibility — MySQL and Redis
package main

import (
	"context"
	"testing"

	"github.com/redis/go-redis/v9"
)

// TestRedisConnect validates that redis.NewClient is callable and
// returns a properly configured client.
func TestRedisConnect(t *testing.T) {
	t.Run("new_client_returns_non_nil", func(t *testing.T) {
		client := connectRedis("localhost:59999")
		if client == nil {
			t.Fatal("expected non-nil *redis.Client")
		}
		client.Close()
	})

	t.Run("ping_returns_error_for_unreachable_host", func(t *testing.T) {
		client := connectRedis("localhost:59999")
		defer client.Close()

		ctx := context.Background()
		_, err := client.Ping(ctx).Result()
		if err == nil {
			t.Fatal("expected ping error for unreachable host")
		}
		t.Logf("ping error (expected): %v", err)
	})
}

// TestRedisOptions validates that redis.Options can be constructed
// with the expected configuration fields.
func TestRedisOptions(t *testing.T) {
	t.Run("options_fields_are_configurable", func(t *testing.T) {
		opts := &redis.Options{
			Addr:     "localhost:6379",
			Password: "secret",
			DB:       1,
		}
		if opts.Addr != "localhost:6379" {
			t.Errorf("Addr = %q, want %q", opts.Addr, "localhost:6379")
		}
		if opts.Password != "secret" {
			t.Errorf("Password = %q, want %q", opts.Password, "secret")
		}
		if opts.DB != 1 {
			t.Errorf("DB = %d, want %d", opts.DB, 1)
		}
		t.Log("Redis Options fields are configurable")
	})
}

// TestPingCommand validates that a PING command can be constructed.
func TestPingCommand(t *testing.T) {
	t.Run("ping_command_constructs_correctly", func(t *testing.T) {
		client := connectRedis("localhost:59999")
		defer client.Close()

		ctx := context.Background()
		cmd := client.Ping(ctx)
		if cmd == nil {
			t.Fatal("expected non-nil StatusCmd from Ping")
		}
		// cmd.Name() returns the command name
		if cmd.Name() != "ping" {
			t.Errorf("command name = %q, want %q", cmd.Name(), "ping")
		}
		t.Log("PING command constructs correctly")
	})
}

// TestSetGetCycle validates that SET and GET commands can be constructed.
func TestSetGetCycle(t *testing.T) {
	t.Run("set_command_constructs_correctly", func(t *testing.T) {
		client := connectRedis("localhost:59999")
		defer client.Close()

		ctx := context.Background()
		cmd := client.Set(ctx, "test-key", "test-value", 0)
		if cmd == nil {
			t.Fatal("expected non-nil StatusCmd from Set")
		}
		if cmd.Name() != "set" {
			t.Errorf("command name = %q, want %q", cmd.Name(), "set")
		}
		t.Log("SET command constructs correctly")
	})

	t.Run("get_command_constructs_correctly", func(t *testing.T) {
		client := connectRedis("localhost:59999")
		defer client.Close()

		ctx := context.Background()
		cmd := client.Get(ctx, "test-key")
		if cmd == nil {
			t.Fatal("expected non-nil StringCmd from Get")
		}
		if cmd.Name() != "get" {
			t.Errorf("command name = %q, want %q", cmd.Name(), "get")
		}
		t.Log("GET command constructs correctly")
	})
}

// TestRedisImportTypes validates that key go-redis types are importable.
func TestRedisImportTypes(t *testing.T) {
	t.Run("redis_types_are_available", func(t *testing.T) {
		info := getRedisClientInfo()
		expectedTypes := []string{"client_type", "options_type", "cmd_type", "status_type", "string_type"}
		for _, key := range expectedTypes {
			if info[key] == "" {
				t.Errorf("type info for %q is empty", key)
			}
		}
		t.Log("go-redis core types import successfully")
	})
}
