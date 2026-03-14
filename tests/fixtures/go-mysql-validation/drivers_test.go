// Package main validates the structure of compat-db/tinygo-drivers.json.
//
// This test ensures the driver compatibility report contains valid entries
// for both go-sql-driver/mysql and go-redis/redis with all required fields.
//
// US-308: Database driver compatibility — MySQL and Redis
package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

// driverError represents a compilation error entry.
type driverError struct {
	Symbol        string `json:"symbol"`
	Type          string `json:"type"`
	StdlibPackage string `json:"stdlibPackage"`
	Description   string `json:"description"`
}

// stdlibDep represents a blocking stdlib dependency.
type stdlibDep struct {
	Package  string   `json:"package"`
	Missing  []string `json:"missing"`
	Severity string   `json:"severity"`
}

// workaround represents a driver workaround entry.
type workaround struct {
	Approach    string   `json:"approach"`
	Description string   `json:"description"`
	Status      string   `json:"status"`
	References  []string `json:"references"`
}

// featureTested represents a tested feature entry.
type featureTested struct {
	Feature    string `json:"feature"`
	GoStatus   string `json:"goStatus"`
	WasmStatus string `json:"wasmStatus"`
}

// driverEntry represents a single driver in the compatibility report.
type driverEntry struct {
	Name              string          `json:"name"`
	ImportPath        string          `json:"importPath"`
	Version           string          `json:"version"`
	Ecosystem         string          `json:"ecosystem"`
	CompileStatus     string          `json:"compileStatus"`
	GoTestStatus      string          `json:"goTestStatus"`
	GoTestCount       int             `json:"goTestCount"`
	Errors            []driverError   `json:"errors"`
	BlockingStdlibDeps []stdlibDep    `json:"blockingStdlibDeps"`
	Workarounds       []workaround    `json:"workarounds"`
	FeaturesTested    []featureTested `json:"featuresTested"`
}

// overlayAssessment represents the database/sql overlay assessment.
type overlayAssessment struct {
	Needed bool   `json:"needed"`
	Reason string `json:"reason"`
}

// driversReport represents the top-level schema of tinygo-drivers.json.
type driversReport struct {
	Compiler            string            `json:"compiler"`
	Target              string            `json:"target"`
	UserStory           string            `json:"userStory"`
	ValidationDate      string            `json:"validationDate"`
	DriverCount         int               `json:"driverCount"`
	DatabaseSqlOverlay  overlayAssessment `json:"databaseSqlOverlay"`
	Drivers             []driverEntry     `json:"drivers"`
}

// driversProjectRoot returns the workspace root (three levels up from fixture dir).
func driversProjectRoot(t *testing.T) string {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot determine test file path")
	}
	return filepath.Join(filepath.Dir(thisFile), "..", "..", "..")
}

// loadDriversJSON reads and parses the tinygo-drivers.json file.
func loadDriversJSON(t *testing.T) driversReport {
	t.Helper()
	jsonPath := filepath.Join(driversProjectRoot(t), "compat-db", "tinygo-drivers.json")
	data, err := os.ReadFile(jsonPath)
	if err != nil {
		t.Fatalf("cannot read compat-db/tinygo-drivers.json: %v", err)
	}
	var report driversReport
	if err := json.Unmarshal(data, &report); err != nil {
		t.Fatalf("invalid JSON in tinygo-drivers.json: %v", err)
	}
	return report
}

// TestDriversJSONSchema validates the structure and field values of
// compat-db/tinygo-drivers.json.
func TestDriversJSONSchema(t *testing.T) {
	report := loadDriversJSON(t)

	if report.Compiler != "tinygo" {
		t.Errorf("compiler = %q, want %q", report.Compiler, "tinygo")
	}
	if report.Target != "wasip2" {
		t.Errorf("target = %q, want %q", report.Target, "wasip2")
	}
	if report.UserStory != "US-308" {
		t.Errorf("userStory = %q, want %q", report.UserStory, "US-308")
	}
	if report.ValidationDate == "" {
		t.Error("validationDate is empty")
	}
	if report.DriverCount != 2 {
		t.Errorf("driverCount = %d, want 2", report.DriverCount)
	}
	if len(report.Drivers) != 2 {
		t.Fatalf("drivers array has %d entries, want 2", len(report.Drivers))
	}
}

// TestDriversContainBothDrivers verifies that both go-sql-driver/mysql
// and go-redis/redis are present in the report.
func TestDriversContainBothDrivers(t *testing.T) {
	report := loadDriversJSON(t)

	expectedImports := map[string]bool{
		"github.com/go-sql-driver/mysql": false,
		"github.com/redis/go-redis/v9":   false,
	}

	for _, driver := range report.Drivers {
		if _, ok := expectedImports[driver.ImportPath]; ok {
			expectedImports[driver.ImportPath] = true
		}
	}

	for importPath, found := range expectedImports {
		if !found {
			t.Errorf("driver %q not found in tinygo-drivers.json", importPath)
		}
	}
}

// TestDriverEntryRequiredFields validates that each driver entry has all required fields.
func TestDriverEntryRequiredFields(t *testing.T) {
	report := loadDriversJSON(t)

	validStatuses := map[string]bool{"pass": true, "fail": true, "partial": true}

	for _, driver := range report.Drivers {
		t.Run(driver.Name, func(t *testing.T) {
			if driver.Name == "" {
				t.Error("name is empty")
			}
			if driver.ImportPath == "" {
				t.Error("importPath is empty")
			}
			if driver.Version == "" {
				t.Error("version is empty")
			}
			if driver.Ecosystem != "go" {
				t.Errorf("ecosystem = %q, want %q", driver.Ecosystem, "go")
			}
			if !validStatuses[driver.CompileStatus] {
				t.Errorf("compileStatus = %q, want one of pass/fail/partial", driver.CompileStatus)
			}
			if !validStatuses[driver.GoTestStatus] {
				t.Errorf("goTestStatus = %q, want one of pass/fail/partial", driver.GoTestStatus)
			}
		})
	}
}

// TestDriverErrorsPresent verifies that drivers with "fail" compileStatus
// have non-empty errors and blockingStdlibDeps arrays.
func TestDriverErrorsPresent(t *testing.T) {
	report := loadDriversJSON(t)

	for _, driver := range report.Drivers {
		if driver.CompileStatus == "fail" {
			t.Run(driver.Name, func(t *testing.T) {
				if len(driver.Errors) == 0 {
					t.Errorf("driver %q has status 'fail' but empty errors array", driver.Name)
				}
				if len(driver.BlockingStdlibDeps) == 0 {
					t.Errorf("driver %q has status 'fail' but empty blockingStdlibDeps array", driver.Name)
				}
			})
		}
	}
}

// TestDriverWorkaroundsPresent verifies that each driver has at least one workaround.
func TestDriverWorkaroundsPresent(t *testing.T) {
	report := loadDriversJSON(t)

	for _, driver := range report.Drivers {
		t.Run(driver.Name, func(t *testing.T) {
			if len(driver.Workarounds) == 0 {
				t.Errorf("driver %q has no workarounds", driver.Name)
			}
		})
	}
}

// TestDriverFeaturesTestedPresent verifies that each driver has at least one tested feature.
func TestDriverFeaturesTestedPresent(t *testing.T) {
	report := loadDriversJSON(t)

	for _, driver := range report.Drivers {
		t.Run(driver.Name, func(t *testing.T) {
			if len(driver.FeaturesTested) == 0 {
				t.Errorf("driver %q has no featuresTested entries", driver.Name)
			}
			for _, ft := range driver.FeaturesTested {
				if ft.Feature == "" {
					t.Error("featureTested has empty feature name")
				}
				if ft.GoStatus == "" {
					t.Error("featureTested has empty goStatus")
				}
				if ft.WasmStatus == "" {
					t.Error("featureTested has empty wasmStatus")
				}
			}
		})
	}
}

// TestNoDuplicateDrivers verifies that each import path appears exactly once.
func TestNoDuplicateDrivers(t *testing.T) {
	report := loadDriversJSON(t)

	seen := make(map[string]bool)
	for _, driver := range report.Drivers {
		if seen[driver.ImportPath] {
			t.Errorf("duplicate driver entry for %q", driver.ImportPath)
		}
		seen[driver.ImportPath] = true
	}
}

// TestDatabaseSqlOverlayAssessment verifies that the databaseSqlOverlay field
// is present and documents the overlay decision (US-308 Phase 5 deliverable).
func TestDatabaseSqlOverlayAssessment(t *testing.T) {
	report := loadDriversJSON(t)

	if report.DatabaseSqlOverlay.Reason == "" {
		t.Error("databaseSqlOverlay.reason is empty; the overlay decision must be documented")
	}
	// Per US-308 assessment: overlay is NOT needed because database/sql compiles fine;
	// the blocking issues are in driver crypto/tls deps.
	if report.DatabaseSqlOverlay.Needed {
		t.Error("databaseSqlOverlay.needed should be false per US-308 assessment")
	}
}

// TestGoTestCountMatchesActualTests verifies that each driver's goTestCount
// field is consistent with the reported goTestStatus. A driver with goTestStatus
// "pass" must have goTestCount > 0.
func TestGoTestCountMatchesActualTests(t *testing.T) {
	report := loadDriversJSON(t)

	for _, driver := range report.Drivers {
		t.Run(driver.Name, func(t *testing.T) {
			if driver.GoTestStatus == "pass" && driver.GoTestCount <= 0 {
				t.Errorf("goTestStatus is %q but goTestCount is %d; expected > 0",
					driver.GoTestStatus, driver.GoTestCount)
			}
		})
	}
}
