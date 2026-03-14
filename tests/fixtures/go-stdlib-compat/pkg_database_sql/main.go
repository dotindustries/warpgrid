package main

import (
	"database/sql"
	"database/sql/driver"
	"fmt"
)

// stubDriver is a minimal driver implementation to test the registration pattern.
type stubDriver struct{}

func (d *stubDriver) Open(name string) (driver.Conn, error) {
	return nil, fmt.Errorf("stub driver: not implemented")
}

func main() {
	// Register a driver
	sql.Register("stub", &stubDriver{})

	// sql.Open — opens a database handle (does not connect)
	db, err := sql.Open("stub", "test://localhost")
	if err != nil {
		fmt.Println("Open error:", err)
		return
	}
	defer db.Close()
	fmt.Println("db opened (stub driver)")

	// Ping will fail because stub driver doesn't implement connections
	err = db.Ping()
	if err != nil {
		fmt.Println("Ping error (expected):", err)
	}

	// sql.Drivers — list registered drivers
	drivers := sql.Drivers()
	fmt.Println("registered drivers:", drivers)
}
