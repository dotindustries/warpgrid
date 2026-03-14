// Package main validates go-sql-driver/mysql compilation and runtime behavior with TinyGo wasip2.
//
// US-308: Database driver compatibility — MySQL and Redis
//
// This program imports database/sql + go-sql-driver/mysql and exercises:
//   - sql.Open("mysql", dsn)
//   - SELECT 1 query
//   - CREATE TABLE, INSERT, SELECT, DROP TABLE sequence
//
// When compiled with TinyGo wasip2, any unsupported stdlib dependencies
// surface as compilation errors. These are documented in compat-db/tinygo-drivers.json.
package main

import (
	"database/sql"
	"fmt"
	"os"

	_ "github.com/go-sql-driver/mysql"
)

// crudQuery pairs a named operation with its SQL statement.
type crudQuery struct {
	name string
	sql  string
	args []any
}

func main() {
	dsn := os.Getenv("MYSQL_DSN")
	if dsn == "" {
		dsn = "testuser:testpass@tcp(localhost:3306)/testdb"
	}

	db, err := connectMySQL(dsn)
	if err != nil {
		fmt.Fprintf(os.Stderr, "connect failed: %v\n", err)
		os.Exit(1)
	}
	defer db.Close()

	if err := runSelectOne(db); err != nil {
		fmt.Fprintf(os.Stderr, "SELECT 1 failed: %v\n", err)
		os.Exit(1)
	}

	if err := runCRUDSequence(db); err != nil {
		fmt.Fprintf(os.Stderr, "CRUD sequence failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("mysql validation: all operations succeeded")
}

// connectMySQL opens a database/sql connection using the MySQL driver.
func connectMySQL(dsn string) (*sql.DB, error) {
	db, err := sql.Open("mysql", dsn)
	if err != nil {
		return nil, fmt.Errorf("sql.Open: %w", err)
	}
	return db, nil
}

// runSelectOne executes SELECT 1 and verifies the result.
func runSelectOne(db *sql.DB) error {
	var result int
	err := db.QueryRow("SELECT 1 AS result").Scan(&result)
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
func runCRUDSequence(db *sql.DB) error {
	queries := getCRUDQueries()

	// CREATE TABLE
	_, err := db.Exec(queries[0].sql)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[0].name, err)
	}
	fmt.Printf("%s: OK\n", queries[0].name)

	// INSERT
	_, err = db.Exec(queries[1].sql, queries[1].args...)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[1].name, err)
	}
	fmt.Printf("%s: OK\n", queries[1].name)

	// SELECT
	var id int
	var name string
	err = db.QueryRow(queries[2].sql).Scan(&id, &name)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[2].name, err)
	}
	if name != "mysql-test-user" {
		return fmt.Errorf("SELECT returned name=%q, expected %q", name, "mysql-test-user")
	}
	fmt.Printf("%s: OK (id=%d, name=%s)\n", queries[2].name, id, name)

	// DROP TABLE
	_, err = db.Exec(queries[3].sql)
	if err != nil {
		return fmt.Errorf("%s: %w", queries[3].name, err)
	}
	fmt.Printf("%s: OK\n", queries[3].name)

	return nil
}

// getCRUDQueries returns the ordered sequence of CRUD operations for MySQL.
func getCRUDQueries() []crudQuery {
	return []crudQuery{
		{
			name: "create_table",
			sql:  "CREATE TABLE IF NOT EXISTS mysql_validation_test (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(255) NOT NULL)",
		},
		{
			name: "insert",
			sql:  "INSERT INTO mysql_validation_test (name) VALUES (?)",
			args: []any{"mysql-test-user"},
		},
		{
			name: "select",
			sql:  "SELECT id, name FROM mysql_validation_test ORDER BY id DESC LIMIT 1",
		},
		{
			name: "drop_table",
			sql:  "DROP TABLE IF EXISTS mysql_validation_test",
		},
	}
}

// getMySQLDriverInfo validates that the MySQL driver registers with database/sql.
// This function exists primarily to force the compiler to resolve the driver import.
func getMySQLDriverInfo() map[string]string {
	return map[string]string{
		"driver_name": "mysql",
		"db_type":     fmt.Sprintf("%T", (*sql.DB)(nil)),
		"row_type":    fmt.Sprintf("%T", (*sql.Row)(nil)),
		"rows_type":   fmt.Sprintf("%T", (*sql.Rows)(nil)),
		"tx_type":     fmt.Sprintf("%T", (*sql.Tx)(nil)),
	}
}
