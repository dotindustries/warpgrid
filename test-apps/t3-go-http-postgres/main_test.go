package main

import (
	"bytes"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func resetState() {
	seedUsers = []User{
		{ID: 1, Name: "Alice Johnson", Email: "alice@example.com"},
		{ID: 2, Name: "Bob Smith", Email: "bob@example.com"},
		{ID: 3, Name: "Carol Williams", Email: "carol@example.com"},
		{ID: 4, Name: "Dave Brown", Email: "dave@example.com"},
		{ID: 5, Name: "Eve Davis", Email: "eve@example.com"},
	}
	nextID = 6
}

func TestGetHealth(t *testing.T) {
	resetState()
	req := httptest.NewRequest(http.MethodGet, "/health", nil)
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	var resp healthResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}
	if resp.Status != "ok" {
		t.Fatalf("expected status ok, got %s", resp.Status)
	}
}

func TestGetUsersReturnsSeedData(t *testing.T) {
	resetState()
	req := httptest.NewRequest(http.MethodGet, "/users", nil)
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusOK {
		t.Fatalf("expected 200, got %d", w.Code)
	}

	ct := w.Header().Get("Content-Type")
	if ct != "application/json" {
		t.Fatalf("expected Content-Type application/json, got %s", ct)
	}

	var users []User
	if err := json.Unmarshal(w.Body.Bytes(), &users); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}
	if len(users) != 5 {
		t.Fatalf("expected 5 seed users, got %d", len(users))
	}
	if users[0].Name != "Alice Johnson" {
		t.Fatalf("expected first user Alice Johnson, got %s", users[0].Name)
	}
	if users[4].Name != "Eve Davis" {
		t.Fatalf("expected last user Eve Davis, got %s", users[4].Name)
	}
}

func TestPostUsersCreatesUser(t *testing.T) {
	resetState()
	body := `{"name":"Test User","email":"test@example.com"}`
	req := httptest.NewRequest(http.MethodPost, "/users", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusCreated {
		t.Fatalf("expected 201, got %d. Body: %s", w.Code, w.Body.String())
	}

	var user User
	if err := json.Unmarshal(w.Body.Bytes(), &user); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}
	if user.Name != "Test User" {
		t.Fatalf("expected name Test User, got %s", user.Name)
	}
	if user.Email != "test@example.com" {
		t.Fatalf("expected email test@example.com, got %s", user.Email)
	}
	if user.ID != 6 {
		t.Fatalf("expected id 6, got %d", user.ID)
	}
}

func TestPostThenGetIncludesNewUser(t *testing.T) {
	resetState()

	// POST a new user
	body := `{"name":"Test User","email":"test@example.com"}`
	req := httptest.NewRequest(http.MethodPost, "/users", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusCreated {
		t.Fatalf("POST expected 201, got %d", w.Code)
	}

	// GET users — should now have 6
	req2 := httptest.NewRequest(http.MethodGet, "/users", nil)
	w2 := httptest.NewRecorder()
	handler(w2, req2)

	var users []User
	if err := json.Unmarshal(w2.Body.Bytes(), &users); err != nil {
		t.Fatalf("invalid JSON: %v", err)
	}
	if len(users) != 6 {
		t.Fatalf("expected 6 users after POST, got %d", len(users))
	}
	if users[5].Name != "Test User" {
		t.Fatalf("expected new user at index 5, got %s", users[5].Name)
	}
}

func TestPostUsersInvalidJSON(t *testing.T) {
	resetState()
	req := httptest.NewRequest(http.MethodPost, "/users", bytes.NewBufferString("not-json"))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for invalid JSON, got %d", w.Code)
	}

	var resp errorResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("invalid error JSON: %v", err)
	}
	if resp.Error != "Invalid JSON" {
		t.Fatalf("expected error 'Invalid JSON', got '%s'", resp.Error)
	}
}

func TestPostUsersMissingFields(t *testing.T) {
	resetState()
	body := `{"name":"No Email"}`
	req := httptest.NewRequest(http.MethodPost, "/users", bytes.NewBufferString(body))
	req.Header.Set("Content-Type", "application/json")
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for missing email, got %d", w.Code)
	}
}

func TestUnknownRouteReturns404(t *testing.T) {
	resetState()
	req := httptest.NewRequest(http.MethodGet, "/nonexistent", nil)
	w := httptest.NewRecorder()
	handler(w, req)

	if w.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d", w.Code)
	}

	var resp errorResponse
	if err := json.Unmarshal(w.Body.Bytes(), &resp); err != nil {
		t.Fatalf("invalid error JSON: %v", err)
	}
	if resp.Error != "Not Found" {
		t.Fatalf("expected error 'Not Found', got '%s'", resp.Error)
	}
}

func TestXAppNameHeader(t *testing.T) {
	resetState()
	req := httptest.NewRequest(http.MethodGet, "/health", nil)
	w := httptest.NewRecorder()
	handler(w, req)

	appName := w.Header().Get("X-App-Name")
	if appName == "" {
		t.Fatal("expected X-App-Name header to be present")
	}
	if appName != "t3-go-http-postgres" {
		t.Fatalf("expected X-App-Name 't3-go-http-postgres', got '%s'", appName)
	}
}

func TestResponseContentType(t *testing.T) {
	resetState()
	endpoints := []struct {
		method string
		path   string
	}{
		{http.MethodGet, "/health"},
		{http.MethodGet, "/users"},
		{http.MethodGet, "/nonexistent"},
	}

	for _, ep := range endpoints {
		req := httptest.NewRequest(ep.method, ep.path, nil)
		w := httptest.NewRecorder()
		handler(w, req)

		ct := w.Header().Get("Content-Type")
		if ct != "application/json" {
			t.Errorf("%s %s: expected Content-Type application/json, got %s", ep.method, ep.path, ct)
		}
	}
}
