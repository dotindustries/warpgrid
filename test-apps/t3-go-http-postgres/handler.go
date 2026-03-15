package main

import (
	"encoding/json"
	"net/http"
)

// seedUsers holds the in-memory user store used by the standalone handler.
var seedUsers []User

// nextID tracks the next auto-increment ID for new users.
var nextID int

// healthResponse is the JSON shape returned by GET /health.
type healthResponse struct {
	Status string `json:"status"`
}

// errorResponse is the JSON shape returned for error conditions.
type errorResponse struct {
	Error string `json:"error"`
}

func init() {
	seedUsers = []User{
		{ID: 1, Name: "Alice Johnson", Email: "alice@example.com"},
		{ID: 2, Name: "Bob Smith", Email: "bob@example.com"},
		{ID: 3, Name: "Carol Williams", Email: "carol@example.com"},
		{ID: 4, Name: "Dave Brown", Email: "dave@example.com"},
		{ID: 5, Name: "Eve Davis", Email: "eve@example.com"},
	}
	nextID = 6
}

// handler is the standalone HTTP handler that routes requests and uses
// in-memory storage. It is used by unit tests and the standalone binary mode.
func handler(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("X-App-Name", "t3-go-http-postgres")
	w.Header().Set("Content-Type", "application/json")

	switch {
	case r.URL.Path == "/health" && r.Method == http.MethodGet:
		json.NewEncoder(w).Encode(healthResponse{Status: "ok"})

	case r.URL.Path == "/users" && r.Method == http.MethodGet:
		json.NewEncoder(w).Encode(seedUsers)

	case r.URL.Path == "/users" && r.Method == http.MethodPost:
		var input struct {
			Name  string `json:"name"`
			Email string `json:"email"`
		}
		if err := json.NewDecoder(r.Body).Decode(&input); err != nil {
			w.WriteHeader(http.StatusBadRequest)
			json.NewEncoder(w).Encode(errorResponse{Error: "Invalid JSON"})
			return
		}
		if input.Name == "" || input.Email == "" {
			w.WriteHeader(http.StatusBadRequest)
			json.NewEncoder(w).Encode(errorResponse{Error: "Missing required fields: name and email"})
			return
		}

		user := User{ID: nextID, Name: input.Name, Email: input.Email}
		nextID++
		seedUsers = append(seedUsers, user)

		w.WriteHeader(http.StatusCreated)
		json.NewEncoder(w).Encode(user)

	default:
		w.WriteHeader(http.StatusNotFound)
		json.NewEncoder(w).Encode(errorResponse{Error: "Not Found"})
	}
}
