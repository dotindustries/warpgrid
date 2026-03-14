package main

import (
	"crypto/tls"
	"fmt"
)

func main() {
	// tls.Config construction
	cfg := &tls.Config{
		MinVersion: tls.VersionTLS12,
		MaxVersion: tls.VersionTLS13,
	}
	fmt.Println("MinVersion:", cfg.MinVersion)
	fmt.Println("MaxVersion:", cfg.MaxVersion)

	// Config.Clone — known to be missing in TinyGo wasip2
	cloned := cfg.Clone()
	fmt.Println("Cloned MinVersion:", cloned.MinVersion)

	// X509KeyPair — known to be missing in TinyGo wasip2
	// Using a minimal self-signed cert/key pair for compilation test only
	certPEM := []byte(`-----BEGIN CERTIFICATE-----
MIIBhTCCASugAwIBAgIQIRi6zePL6mKjOipn+dNuaTAKBggqhkjOPQQDAjASMRAw
DgYDVQQKEwdBY21lIENvMB4XDTE3MTAyMDE5NDMwNloXDTE4MTAyMDE5NDMwNlow
EjEQMA4GA1UEChMHQWNtZSBDbzBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABD0d
7VNhbWvZLWPuj/RtHFjvtJBEwOkhbN/BnnE8rnZR8+sbwnc/KhCk3FhnpHZnQz7B
5aETbbIgmuj6wo2UV2CjYzBhMA4GA1UdDwEB/wQEAwICpDATBgNVHSUEDDAKBggr
BgEFBQcDATAPBgNVHRMBAf8EBTADAQH/MCkGA1UdEQQiMCCCDmxvY2FsaG9zdDo1
NDUzgg4xMjcuMC4wLjE6NTQ1MzAKBggqhkjOPQQDAgNIADBFAiEA2wpSek3WlBfl
/co2Z69sFlOmCwmGiJkOLMBMlJkkqSYCIHEXJSaBkWGsQMkGR9fIskGrVRBbPsKV
BfkR4eJhjhz6
-----END CERTIFICATE-----`)
	keyPEM := []byte(`-----BEGIN EC PRIVATE KEY-----
MHQCAQEEIIrYSSNQFaA2Hwf583QmKbyavkgoftpCYFjICvbQNUuqoAcGBSuBBAAi
oWQDYgAEPR3tU2Fta9ktY+6P9G0cWO+0kETA6SFs38GecTyudlHz6xvCdz8qEKTc
WGekdmdDPsHloRNtsiCa6PrCjZRXYA==
-----END EC PRIVATE KEY-----`)
	_, err := tls.X509KeyPair(certPEM, keyPEM)
	if err != nil {
		fmt.Println("X509KeyPair error (expected in TinyGo):", err)
	} else {
		fmt.Println("X509KeyPair: ok")
	}
}
