package main_test

import (
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"testing"
)

// expectedPackages lists all 20 stdlib packages that must be audited.
// Each key is the directory name under tests/fixtures/go-stdlib-compat/,
// and the value is the Go import path being exercised.
var expectedPackages = map[string]string{
	"pkg_fmt":             "fmt",
	"pkg_strings":         "strings",
	"pkg_strconv":         "strconv",
	"pkg_encoding_json":   "encoding/json",
	"pkg_encoding_base64": "encoding/base64",
	"pkg_crypto_sha256":   "crypto/sha256",
	"pkg_crypto_tls":      "crypto/tls",
	"pkg_math":            "math",
	"pkg_sort":            "sort",
	"pkg_bytes":           "bytes",
	"pkg_io":              "io",
	"pkg_os":              "os",
	"pkg_net":             "net",
	"pkg_net_http":        "net/http",
	"pkg_database_sql":    "database/sql",
	"pkg_context":         "context",
	"pkg_sync":            "sync",
	"pkg_time":            "time",
	"pkg_regexp":          "regexp",
	"pkg_log":             "log",
}

// fixtureRoot returns the absolute path to the go-stdlib-compat directory.
func fixtureRoot(t *testing.T) string {
	t.Helper()
	_, thisFile, _, ok := runtime.Caller(0)
	if !ok {
		t.Fatal("cannot determine test file path")
	}
	return filepath.Dir(thisFile)
}

// projectRoot returns the workspace root (three levels up from fixture dir).
func projectRoot(t *testing.T) string {
	t.Helper()
	return filepath.Join(fixtureRoot(t), "..", "..", "..")
}

// TestAllPackageDirsExist verifies that all 20 pkg_* subdirectories exist
// and each contains a main.go file.
func TestAllPackageDirsExist(t *testing.T) {
	root := fixtureRoot(t)
	for dir := range expectedPackages {
		dir := dir
		t.Run(dir, func(t *testing.T) {
			mainGo := filepath.Join(root, dir, "main.go")
			info, err := os.Stat(mainGo)
			if err != nil {
				t.Fatalf("missing %s/main.go: %v", dir, err)
			}
			if info.IsDir() {
				t.Fatalf("%s/main.go is a directory, not a file", dir)
			}
		})
	}
}

// TestStandardGoBuild runs `go build` on each pkg_* subdirectory to confirm
// all 20 programs compile with the standard Go compiler.
func TestStandardGoBuild(t *testing.T) {
	root := fixtureRoot(t)
	for dir := range expectedPackages {
		dir := dir
		t.Run(dir, func(t *testing.T) {
			t.Parallel()
			pkgDir := filepath.Join(root, dir)
			tmpOut := filepath.Join(t.TempDir(), dir)
			cmd := exec.Command("go", "build", "-o", tmpOut, ".")
			cmd.Dir = pkgDir
			out, err := cmd.CombinedOutput()
			if err != nil {
				t.Fatalf("go build failed for %s:\n%s", dir, string(out))
			}
		})
	}
}

// compatResult represents a single package entry in tinygo-stdlib.json.
type compatResult struct {
	Name           string   `json:"name"`
	ImportPath     string   `json:"importPath"`
	CompileStatus  string   `json:"compileStatus"`
	Errors         []string `json:"errors"`
	ErrorCount     int      `json:"errorCount"`
	MissingSymbols []string `json:"missingSymbols"`
	Notes          string   `json:"notes"`
}

// compatReport represents the top-level schema of tinygo-stdlib.json.
type compatReport struct {
	Compiler        string         `json:"compiler"`
	CompilerVersion string         `json:"compilerVersion"`
	Target          string         `json:"target"`
	TestedAt        string         `json:"testedAt"`
	GoVersion       string         `json:"goVersion"`
	UserStory       string         `json:"userStory"`
	PackageCount    int            `json:"packageCount"`
	Packages        []compatResult `json:"packages"`
}

// loadCompatJSON reads and parses the tinygo-stdlib.json file.
func loadCompatJSON(t *testing.T) compatReport {
	t.Helper()
	jsonPath := filepath.Join(projectRoot(t), "compat-db", "tinygo-stdlib.json")
	data, err := os.ReadFile(jsonPath)
	if err != nil {
		t.Fatalf("cannot read compat-db/tinygo-stdlib.json: %v", err)
	}
	var report compatReport
	if err := json.Unmarshal(data, &report); err != nil {
		t.Fatalf("invalid JSON in tinygo-stdlib.json: %v", err)
	}
	return report
}

// TestCompatJSONSchema validates the structure and field values of
// compat-db/tinygo-stdlib.json.
func TestCompatJSONSchema(t *testing.T) {
	report := loadCompatJSON(t)

	// Validate metadata fields
	if report.Compiler != "tinygo" {
		t.Errorf("compiler = %q, want %q", report.Compiler, "tinygo")
	}
	if report.Target != "wasip2" {
		t.Errorf("target = %q, want %q", report.Target, "wasip2")
	}
	if report.TestedAt == "" {
		t.Error("testedAt is empty")
	}
	if report.CompilerVersion == "" {
		t.Error("compilerVersion is empty")
	}
	if report.GoVersion == "" {
		t.Error("goVersion is empty")
	}
	if report.UserStory != "US-302" {
		t.Errorf("userStory = %q, want %q", report.UserStory, "US-302")
	}

	// Validate package count
	if report.PackageCount != 20 {
		t.Errorf("packageCount = %d, want 20", report.PackageCount)
	}
	if len(report.Packages) != 20 {
		t.Fatalf("packages array has %d entries, want 20", len(report.Packages))
	}

	// Validate each package entry has required fields and valid status
	validStatuses := map[string]bool{"pass": true, "fail": true, "partial": true}
	for _, pkg := range report.Packages {
		t.Run(pkg.Name, func(t *testing.T) {
			if pkg.Name == "" {
				t.Error("name is empty")
			}
			if pkg.ImportPath == "" {
				t.Error("importPath is empty")
			}
			if !validStatuses[pkg.CompileStatus] {
				t.Errorf("compileStatus = %q, want one of pass/fail/partial", pkg.CompileStatus)
			}
		})
	}
}

// TestAllPackagesRepresented cross-references the 20 expected package names
// against the entries in tinygo-stdlib.json.
func TestAllPackagesRepresented(t *testing.T) {
	report := loadCompatJSON(t)

	found := make(map[string]bool)
	for _, pkg := range report.Packages {
		found[pkg.ImportPath] = true
	}

	for _, importPath := range expectedPackages {
		if !found[importPath] {
			t.Errorf("package %q not found in tinygo-stdlib.json", importPath)
		}
	}
}

// TestErrorDetailsPresent verifies that packages with "fail" status have
// a non-empty errors array.
func TestErrorDetailsPresent(t *testing.T) {
	report := loadCompatJSON(t)

	for _, pkg := range report.Packages {
		if pkg.CompileStatus == "fail" {
			t.Run(pkg.Name, func(t *testing.T) {
				if len(pkg.Errors) == 0 {
					t.Errorf("package %q has status 'fail' but empty errors array", pkg.Name)
				}
				if pkg.ErrorCount == 0 {
					t.Errorf("package %q has status 'fail' but errorCount is 0", pkg.Name)
				}
			})
		}
	}
}
