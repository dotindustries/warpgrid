// Package main implements a Go HTTP handler with Postgres (pgx) for WarpGrid.
//
// This is the reference Go application for the T3 integration test.
// When compiled with patched TinyGo (warp-tinygo) targeting wasip2,
// net.Dial routes through the WarpGrid database proxy shim and DNS
// resolution goes through the WarpGrid DNS shim.
//
// Build: warp pack --lang go  (requires patched TinyGo from Domain 3)
// Target: wasm32-wasip2
//
// The HTTP handler implements:
//   GET  /users     — returns all users as JSON
//   POST /users     — creates a new user, returns 201
//
// Database connectivity flows through the WarpGrid shim chain:
//   net.Dial("tcp", "db.test.warp.local:5432")
//     → DNS shim resolves "db.test.warp.local" to service registry IP
//     → connect() routed through database proxy shim
//     → send/recv pass raw Postgres wire protocol bytes through proxy
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"log"
	"net/http"
	"os"

	"github.com/jackc/pgx/v5"
)

// User represents a row in the test_users table.
type User struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}

func main() {
	connStr := os.Getenv("DATABASE_URL")
	if connStr == "" {
		connStr = "postgres://testuser@db.test.warp.local:5432/testdb"
	}

	http.HandleFunc("/users", func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet:
			handleGetUsers(w, r, connStr)
		case http.MethodPost:
			handlePostUser(w, r, connStr)
		default:
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
		}
	})

	log.Println("listening on :8080")
	log.Fatal(http.ListenAndServe(":8080", nil))
}

func handleGetUsers(w http.ResponseWriter, _ *http.Request, connStr string) {
	conn, err := pgx.Connect(context.Background(), connStr)
	if err != nil {
		http.Error(w, fmt.Sprintf("db connect: %v", err), http.StatusServiceUnavailable)
		return
	}
	defer conn.Close(context.Background())

	rows, err := conn.Query(context.Background(), "SELECT id, name FROM test_users ORDER BY id")
	if err != nil {
		http.Error(w, fmt.Sprintf("query: %v", err), http.StatusInternalServerError)
		return
	}
	defer rows.Close()

	var users []User
	for rows.Next() {
		var u User
		if err := rows.Scan(&u.ID, &u.Name); err != nil {
			http.Error(w, fmt.Sprintf("scan: %v", err), http.StatusInternalServerError)
			return
		}
		users = append(users, u)
	}

	w.Header().Set("Content-Type", "application/json")
	if err := json.NewEncoder(w).Encode(users); err != nil {
		http.Error(w, fmt.Sprintf("encode: %v", err), http.StatusInternalServerError)
	}
}

func handlePostUser(w http.ResponseWriter, r *http.Request, connStr string) {
	var input struct {
		Name string `json:"name"`
	}
	if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
		http.Error(w, "invalid json", http.StatusBadRequest)
		return
	}

	conn, err := pgx.Connect(context.Background(), connStr)
	if err != nil {
		http.Error(w, fmt.Sprintf("db connect: %v", err), http.StatusServiceUnavailable)
		return
	}
	defer conn.Close(context.Background())

	var id int
	err = conn.QueryRow(context.Background(),
		"INSERT INTO test_users (name) VALUES ($1) RETURNING id", input.Name,
	).Scan(&id)
	if err != nil {
		http.Error(w, fmt.Sprintf("insert: %v", err), http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusCreated)
	if err := json.NewEncoder(w).Encode(User{ID: id, Name: input.Name}); err != nil {
		http.Error(w, fmt.Sprintf("encode: %v", err), http.StatusInternalServerError)
	}
}
