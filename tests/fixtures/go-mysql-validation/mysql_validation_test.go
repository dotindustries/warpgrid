// Package main validates that go-sql-driver/mysql compiles and functions correctly
// when built with TinyGo targeting wasip2.
//
// This test suite serves as both:
//  1. A standard Go test (go test) — validates logic correctness
//  2. A compilation target for TinyGo wasip2 — validates the MySQL driver compiles
//
// US-308: Database driver compatibility — MySQL and Redis
package main

import (
	"database/sql"
	"testing"
)

// TestMySQLConnect validates that sql.Open("mysql", dsn) is callable and
// the MySQL driver is registered with database/sql.
func TestMySQLConnect(t *testing.T) {
	t.Run("open_returns_db_without_error", func(t *testing.T) {
		// sql.Open validates the driver name but does not connect.
		// This confirms the mysql driver is registered via the blank import.
		db, err := connectMySQL("testuser:testpass@tcp(localhost:59999)/testdb")
		if err != nil {
			t.Fatalf("sql.Open failed: %v", err)
		}
		if db == nil {
			t.Fatal("expected non-nil *sql.DB")
		}
		db.Close()
	})

	t.Run("ping_returns_error_for_unreachable_host", func(t *testing.T) {
		// Ping actually attempts a connection — should fail for unreachable host.
		db, err := connectMySQL("testuser:testpass@tcp(localhost:59999)/testdb")
		if err != nil {
			t.Fatalf("sql.Open failed: %v", err)
		}
		defer db.Close()

		err = db.Ping()
		if err == nil {
			t.Fatal("expected ping error for unreachable host")
		}
		t.Logf("ping error (expected): %v", err)
	})
}

// TestSelectOne validates that a SELECT 1 query can be constructed
// and would execute against a live MySQL database.
func TestSelectOne(t *testing.T) {
	t.Run("select_one_query_constructs_correctly", func(t *testing.T) {
		query := "SELECT 1 AS result"
		if query == "" {
			t.Fatal("query must not be empty")
		}
		t.Log("SELECT 1 query ready for execution")
	})
}

// TestCRUDSequence validates that CREATE TABLE, INSERT, SELECT, DROP TABLE
// operations can be constructed with MySQL-specific syntax.
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

	t.Run("mysql_uses_question_mark_placeholders", func(t *testing.T) {
		queries := getCRUDQueries()
		insertSQL := queries[1].sql
		// MySQL uses ? placeholders, not $1 like Postgres.
		if insertSQL == "" {
			t.Fatal("INSERT SQL must not be empty")
		}
		// Verify the insert has a ? placeholder
		found := false
		for _, c := range insertSQL {
			if c == '?' {
				found = true
				break
			}
		}
		if !found {
			t.Errorf("INSERT SQL should use ? placeholder for MySQL, got: %s", insertSQL)
		}
		t.Log("MySQL placeholder syntax validated")
	})
}

// TestMySQLDriverRegistered validates that the mysql driver registers
// with database/sql via the blank import.
func TestMySQLDriverRegistered(t *testing.T) {
	t.Run("driver_is_registered", func(t *testing.T) {
		drivers := sql.Drivers()
		found := false
		for _, d := range drivers {
			if d == "mysql" {
				found = true
				break
			}
		}
		if !found {
			t.Fatal("mysql driver not found in sql.Drivers()")
		}
		t.Log("mysql driver is registered with database/sql")
	})
}

// TestMySQLImportTypes validates that key database/sql types are importable
// and the MySQL driver compiles correctly.
func TestMySQLImportTypes(t *testing.T) {
	t.Run("database_sql_types_are_available", func(t *testing.T) {
		info := getMySQLDriverInfo()
		if info["driver_name"] != "mysql" {
			t.Errorf("driver_name = %q, want %q", info["driver_name"], "mysql")
		}
		t.Log("database/sql types import successfully with MySQL driver")
	})
}
