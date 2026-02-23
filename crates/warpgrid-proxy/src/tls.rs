//! TLS termination configuration for the service mesh.
//!
//! Manages TLS certificates and configuration for incoming
//! connections. Supports SNI-based routing.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::debug;

/// A TLS certificate entry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TlsCert {
    /// Server name (SNI hostname).
    pub server_name: String,
    /// PEM-encoded certificate chain.
    pub cert_pem: String,
    /// PEM-encoded private key (never serialized to clients).
    #[serde(skip_serializing)]
    pub key_pem: String,
    /// Whether this cert is the default (used when SNI doesn't match).
    pub is_default: bool,
}

/// Manages TLS certificates for SNI-based routing.
pub struct TlsTerminator {
    certs: Arc<RwLock<HashMap<String, TlsCert>>>,
    default_server_name: Option<String>,
}

impl TlsTerminator {
    pub fn new() -> Self {
        Self {
            certs: Arc::new(RwLock::new(HashMap::new())),
            default_server_name: None,
        }
    }

    /// Add or update a TLS certificate.
    pub fn upsert_cert(&mut self, cert: TlsCert) {
        let server_name = cert.server_name.clone();
        if cert.is_default {
            self.default_server_name = Some(server_name.clone());
        }
        let mut certs = self.certs.write().expect("tls lock");
        debug!(server_name = %server_name, "upserted TLS cert");
        certs.insert(server_name, cert);
    }

    /// Remove a TLS certificate.
    pub fn remove_cert(&mut self, server_name: &str) {
        let mut certs = self.certs.write().expect("tls lock");
        certs.remove(server_name);
        if self.default_server_name.as_deref() == Some(server_name) {
            self.default_server_name = None;
        }
    }

    /// Resolve a certificate by SNI server name.
    pub fn resolve(&self, server_name: &str) -> Option<TlsCert> {
        let certs = self.certs.read().expect("tls lock");

        // Exact match first.
        if let Some(cert) = certs.get(server_name) {
            return Some(cert.clone());
        }

        // Wildcard match: *.example.com matches foo.example.com.
        for (pattern, cert) in certs.iter() {
            if let Some(suffix) = pattern.strip_prefix("*.")
                && server_name.ends_with(suffix)
                && server_name.len() > suffix.len() + 1
                && !server_name[..server_name.len() - suffix.len() - 1].contains('.')
            {
                return Some(cert.clone());
            }
        }

        // Fall back to default.
        self.default_server_name
            .as_ref()
            .and_then(|name| certs.get(name).cloned())
    }

    /// List all registered server names.
    pub fn list_server_names(&self) -> Vec<String> {
        let certs = self.certs.read().expect("tls lock");
        certs.keys().cloned().collect()
    }
}

impl Default for TlsTerminator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cert(name: &str, is_default: bool) -> TlsCert {
        TlsCert {
            server_name: name.to_string(),
            cert_pem: format!("cert-{name}"),
            key_pem: format!("key-{name}"),
            is_default,
        }
    }

    #[test]
    fn exact_match() {
        let mut term = TlsTerminator::new();
        term.upsert_cert(make_cert("api.example.com", false));

        let cert = term.resolve("api.example.com").unwrap();
        assert_eq!(cert.server_name, "api.example.com");
    }

    #[test]
    fn wildcard_match() {
        let mut term = TlsTerminator::new();
        term.upsert_cert(make_cert("*.example.com", false));

        let cert = term.resolve("api.example.com").unwrap();
        assert_eq!(cert.server_name, "*.example.com");
    }

    #[test]
    fn wildcard_does_not_match_deep_subdomain() {
        let mut term = TlsTerminator::new();
        term.upsert_cert(make_cert("*.example.com", false));

        // *.example.com should NOT match sub.api.example.com.
        assert!(term.resolve("sub.api.example.com").is_none());
    }

    #[test]
    fn falls_back_to_default() {
        let mut term = TlsTerminator::new();
        term.upsert_cert(make_cert("default.local", true));

        let cert = term.resolve("unknown.host").unwrap();
        assert_eq!(cert.server_name, "default.local");
    }

    #[test]
    fn returns_none_when_empty() {
        let term = TlsTerminator::new();
        assert!(term.resolve("anything").is_none());
    }

    #[test]
    fn remove_cert_works() {
        let mut term = TlsTerminator::new();
        term.upsert_cert(make_cert("api.example.com", true));
        term.remove_cert("api.example.com");

        assert!(term.resolve("api.example.com").is_none());
    }

    #[test]
    fn key_not_serialized() {
        let cert = make_cert("test", false);
        let json = serde_json::to_string(&cert).unwrap();
        assert!(!json.contains("key-test"));
    }
}
