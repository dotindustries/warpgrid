// Package main validates pgx/v5 compilation and runtime behavior with TinyGo wasip2.
//
// US-305: Validate pgx Postgres driver over patched net.Dial
//
// This program imports pgx/v5 and exercises:
//   - pgx.Connect(ctx, connString)
//   - SELECT 1 query
//   - CREATE TABLE, INSERT, SELECT, DROP TABLE sequence
//
// When compiled with TinyGo wasip2, any unsupported stdlib dependencies
// surface as compilation errors. These are documented in compat-db/tinygo-pgx.json.
package main

import (
	"context"
	"fmt"
	"os"

	"github.com/jackc/pgx/v5"
)

// crudQuery pairs a named operation with its SQL statement.
type crudQuery struct {
	name string
	sql  string
	args []any
}

func main() {
	connStr := os.Getenv("DATABASE_URL")
	if connStr == "" {
		connStr = "postgres://testuser@localhost:5432/testdb"
	}

	ctx := context.Background()

	conn, err := connectPostgres(ctx, connStr)
	if err != nil {
		fmt.Fprintf(os.Stderr, "connect failed: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close(ctx)

	if err := runSelectOne(ctx, conn); err != nil {
		fmt.Fprintf(os.Stderr, "SELECT 1 failed: %v\n", err)
		os.Exit(1)
	}

	if err := runCRUDSequence(ctx, conn); err != nil {
		fmt.Fprintf(os.Stderr, "CRUD sequence failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("pgx validation: all operations succeeded")
}

// connectPostgres establishes a pgx connection to the given Postgres instance.
func connectPostgres(ctx context.Context, connStr string) (*pgx.Conn, error) {
	conn, err := pgx.Connect(ctx, connStr)
	if err != nil {
		return nil, fmt.Errorf("pgx.Connect: %w", err)
	}
	return conn, nil
}

// runSelectOne executes SELECT 1 and verifies the result.
func runSelectOne(ctx context.Context, conn *pgx.Conn) error {
	var result int
	err := conn.QueryRow(ctx, "SELECT 1 AS result").Scan(&result)
	if err != nil {
		return fmt.Errorf("SELECT 1: %w", err)
	}
	if result != 1 {
		return fmt.Errorf("SELECT 1 returned %d, expected 1", result)
	}
	fmt.Println("SELECT 1: OK")
	return nil
}

// runCRUDSequence executes a full CREATE TABLE → INSERT → SELECT → DROP TABLE cycle.
func runCRUDSequence(ctx context.Context, conn *pgx.Conn) error {
	queries := getCRUDQueries()

	// CREATE TABLE
	_, err := conn.Exec(ctx, queries[0].sql)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[0].name, err)
	}
	fmt.Printf("%s: OK\n", queries[0].name)

	// INSERT
	_, err = conn.Exec(ctx, queries[1].sql, queries[1].args...)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[1].name, err)
	}
	fmt.Printf("%s: OK\n", queries[1].name)

	// SELECT
	var id int
	var name string
	err = conn.QueryRow(ctx, queries[2].sql).Scan(&id, &name)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[2].name, err)
	}
	if name != "pgx-test-user" {
		return fmt.Errorf("SELECT returned name=%q, expected %q", name, "pgx-test-user")
	}
	fmt.Printf("%s: OK (id=%d, name=%s)\n", queries[2].name, id, name)

	// DROP TABLE
	_, err = conn.Exec(ctx, queries[3].sql)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[3].name, err)
	}
	fmt.Printf("%s: OK\n", queries[3].name)

	return nil
}

// getCRUDQueries returns the ordered sequence of CRUD operations.
func getCRUDQueries() []crudQuery {
	return []crudQuery{
		{
			name: "create_table",
			sql:  "CREATE TABLE IF NOT EXISTS pgx_validation_test (id SERIAL PRIMARY KEY, name TEXT NOT NULL)",
		},
		{
			name: "insert",
			sql:  "INSERT INTO pgx_validation_test (name) VALUES ($1)",
			args: []any{"pgx-test-user"},
		},
		{
			name: "select",
			sql:  "SELECT id, name FROM pgx_validation_test ORDER BY id DESC LIMIT 1",
		},
		{
			name: "drop_table",
			sql:  "DROP TABLE IF EXISTS pgx_validation_test",
		},
	}
}

// getPgxTypeInfo validates that core pgx types are importable.
// This function exists primarily to force the compiler to resolve pgx type imports.
func getPgxTypeInfo() map[string]string {
	return map[string]string{
		"conn_type":       fmt.Sprintf("%T", (*pgx.Conn)(nil)),
		"rows_type":       fmt.Sprintf("%T", (*pgx.Rows)(nil)),
		"conn_config_type": fmt.Sprintf("%T", (*pgx.ConnConfig)(nil)),
	}
}
