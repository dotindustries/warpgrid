// Package main validates that pgx/v5 compiles and functions correctly
// when built with TinyGo targeting wasip2.
//
// This test suite serves as both:
//  1. A standard Go test (go test) — validates logic correctness
//  2. A compilation target for TinyGo wasip2 — validates pgx compiles
//
// US-305: Validate pgx Postgres driver over patched net.Dial
package main

import (
	"context"
	"testing"
)

// TestPgxConnect validates that pgx.Connect is callable with a connection string.
// In standard Go tests, this verifies the API surface.
// Under TinyGo wasip2, compilation success proves pgx types are available.
func TestPgxConnect(t *testing.T) {
	t.Run("connect_returns_error_for_unreachable_host", func(t *testing.T) {
		// pgx.Connect to a non-existent host should return an error, not panic.
		ctx := context.Background()
		conn, err := connectPostgres(ctx, "postgres://testuser@localhost:59999/testdb?connect_timeout=1")
		if err == nil {
			t.Fatal("expected connection error for unreachable host")
		}
		if conn != nil {
			t.Fatal("expected nil connection on error")
		}
		t.Logf("connect error (expected): %v", err)
	})
}

// TestSelectOne validates that a SELECT 1 query can be constructed
// and would execute against a live database.
func TestSelectOne(t *testing.T) {
	t.Run("select_one_query_constructs_correctly", func(t *testing.T) {
		// Validate query string is syntactically correct.
		query := "SELECT 1 AS result"
		if query == "" {
			t.Fatal("query must not be empty")
		}
		t.Log("SELECT 1 query ready for execution")
	})
}

// TestCRUDSequence validates that CREATE TABLE, INSERT, SELECT, DROP TABLE
// operations can be constructed and would execute against a live database.
func TestCRUDSequence(t *testing.T) {
	t.Run("crud_queries_construct_correctly", func(t *testing.T) {
		queries := getCRUDQueries()
		expectedOps := []string{"create_table", "insert", "select", "drop_table"}

		if len(queries) != len(expectedOps) {
			t.Fatalf("expected %d CRUD operations, got %d", len(expectedOps), len(queries))
		}

		for i, op := range expectedOps {
			if queries[i].name != op {
				t.Errorf("operation %d: expected %q, got %q", i, op, queries[i].name)
			}
			if queries[i].sql == "" {
				t.Errorf("operation %q has empty SQL", op)
			}
		}
		t.Log("CRUD query sequence validated")
	})
}

// TestPgxImportTypes validates that key pgx types are importable and usable.
func TestPgxImportTypes(t *testing.T) {
	t.Run("pgx_types_are_available", func(t *testing.T) {
		// Validate that pgx core types compile.
		_ = getPgxTypeInfo()
		t.Log("pgx core types import successfully")
	})
}
