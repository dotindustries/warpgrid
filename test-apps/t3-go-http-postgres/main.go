// T3 Integration Test: Go HTTP handler with Postgres via WarpGrid shims.
//
// This handler demonstrates the intended Go HTTP + Postgres pattern on WarpGrid:
// - HTTP routing via standard net/http (mapped to wasi:http by warpgrid/net/http overlay)
// - Database access via warpgrid:shim/database-proxy WIT imports (abstracted by pgx)
//
// Routes:
//
//	GET  /users      — list all users from test_users table
//	POST /users      — insert a new user, return 201
//	GET  /health     — health check
//
// Environment:
//
//	DB_HOST           — Postgres host (default: db.test.warp.local)
//	DB_PORT           — Postgres port (default: 5432)
//	DB_NAME           — database name (default: testdb)
//	DB_USER           — database user (default: testuser)
//
// Dependencies (upstream user stories):
//
//	US-305: pgx Postgres driver over patched net.Dial
//	US-307: warpgrid/net/http overlay — request/response round-trip
//	US-310: warp pack --lang go integration
package main

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strconv"
)

// User represents a row in the test_users table.
type User struct {
	ID    int    `json:"id"`
	Name  string `json:"name"`
	Email string `json:"email"`
}

// createUserRequest is the expected POST /users request body.
type createUserRequest struct {
	Name  string `json:"name"`
	Email string `json:"email"`
}

// errorResponse is the standard error response body.
type errorResponse struct {
	Error string `json:"error"`
}

// healthResponse is the GET /health response body.
type healthResponse struct {
	Status string `json:"status"`
}

// seedUsers provides in-memory seed data matching test-infra/seed.sql.
// When pgx + database proxy (US-305) is ready, this will be replaced
// with actual Postgres queries.
var seedUsers = []User{
	{ID: 1, Name: "Alice Johnson", Email: "alice@example.com"},
	{ID: 2, Name: "Bob Smith", Email: "bob@example.com"},
	{ID: 3, Name: "Carol Williams", Email: "carol@example.com"},
	{ID: 4, Name: "Dave Brown", Email: "dave@example.com"},
	{ID: 5, Name: "Eve Davis", Email: "eve@example.com"},
}

var nextID = 6

func writeJSON(w http.ResponseWriter, status int, data any) {
	body, err := json.Marshal(data)
	if err != nil {
		w.WriteHeader(http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("X-App-Name", envOrDefault("APP_NAME", "t3-go-http-postgres"))
	w.WriteHeader(status)
	w.Write(body)
}

func handleGetUsers(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, seedUsers)
}

func handlePostUsers(w http.ResponseWriter, r *http.Request) {
	var req createUserRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, http.StatusBadRequest, errorResponse{Error: "Invalid JSON"})
		return
	}

	if req.Name == "" || req.Email == "" {
		writeJSON(w, http.StatusBadRequest, errorResponse{Error: "name and email are required"})
		return
	}

	user := User{
		ID:    nextID,
		Name:  req.Name,
		Email: req.Email,
	}
	nextID++
	seedUsers = append(seedUsers, user)

	writeJSON(w, http.StatusCreated, user)
}

func handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, healthResponse{Status: "ok"})
}

func handler(w http.ResponseWriter, r *http.Request) {
	switch {
	case r.URL.Path == "/users" && r.Method == http.MethodGet:
		handleGetUsers(w, r)
	case r.URL.Path == "/users" && r.Method == http.MethodPost:
		handlePostUsers(w, r)
	case r.URL.Path == "/health":
		handleHealth(w, r)
	default:
		writeJSON(w, http.StatusNotFound, errorResponse{Error: "Not Found"})
	}
}

func envOrDefault(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func main() {
	port := envOrDefault("PORT", "8080")
	portNum, err := strconv.Atoi(port)
	if err != nil {
		fmt.Fprintf(os.Stderr, "invalid PORT: %s\n", port)
		os.Exit(1)
	}

	addr := fmt.Sprintf(":%d", portNum)
	http.HandleFunc("/", handler)

	fmt.Fprintf(os.Stderr, "Server listening on %s\n", addr)
	if err := http.ListenAndServe(addr, nil); err != nil {
		fmt.Fprintf(os.Stderr, "server error: %v\n", err)
		os.Exit(1)
	}
}
