//! Request routing — resolves service names to backend instances.
//!
//! The router maintains a mapping from virtual service names to
//! their backend endpoints. When a request arrives for a service,
//! the router selects a backend using round-robin load balancing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use tracing::debug;

/// A backend endpoint that can serve traffic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Backend {
    pub node_id: String,
    pub address: String,
    pub port: u16,
    pub healthy: bool,
}

impl Backend {
    /// Full address string.
    pub fn endpoint(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }
}

/// Internal state for a single service.
struct ServiceEntry {
    backends: Vec<Backend>,
    counter: AtomicUsize,
}

/// Routes requests to backend instances using round-robin.
pub struct Router {
    services: Arc<RwLock<HashMap<String, ServiceEntry>>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            services: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register or update backends for a service.
    pub fn update_service(&self, service_name: &str, backends: Vec<Backend>) {
        let mut services = self.services.write().expect("services lock");
        debug!(
            service = service_name,
            count = backends.len(),
            "updated service backends"
        );
        services.insert(
            service_name.to_string(),
            ServiceEntry {
                backends,
                counter: AtomicUsize::new(0),
            },
        );
    }

    /// Remove a service entirely.
    pub fn remove_service(&self, service_name: &str) {
        let mut services = self.services.write().expect("services lock");
        services.remove(service_name);
    }

    /// Select the next healthy backend for a service (round-robin).
    pub fn next_backend(&self, service_name: &str) -> Option<Backend> {
        let services = self.services.read().expect("services lock");
        let entry = services.get(service_name)?;

        let healthy: Vec<&Backend> = entry.backends.iter().filter(|b| b.healthy).collect();
        if healthy.is_empty() {
            return None;
        }

        let idx = entry.counter.fetch_add(1, Ordering::Relaxed) % healthy.len();
        Some(healthy[idx].clone())
    }

    /// Get all backends for a service (healthy and unhealthy).
    pub fn get_backends(&self, service_name: &str) -> Vec<Backend> {
        let services = self.services.read().expect("services lock");
        services
            .get(service_name)
            .map(|e| e.backends.clone())
            .unwrap_or_default()
    }

    /// List all registered service names.
    pub fn list_services(&self) -> Vec<String> {
        let services = self.services.read().expect("services lock");
        services.keys().cloned().collect()
    }

    /// Mark a specific backend as unhealthy.
    pub fn mark_unhealthy(&self, service_name: &str, endpoint: &str) {
        let mut services = self.services.write().expect("services lock");
        if let Some(entry) = services.get_mut(service_name) {
            for backend in &mut entry.backends {
                if backend.endpoint() == endpoint {
                    backend.healthy = false;
                    debug!(
                        service = service_name,
                        endpoint,
                        "marked backend unhealthy"
                    );
                }
            }
        }
    }

    /// Mark a specific backend as healthy.
    pub fn mark_healthy(&self, service_name: &str, endpoint: &str) {
        let mut services = self.services.write().expect("services lock");
        if let Some(entry) = services.get_mut(service_name) {
            for backend in &mut entry.backends {
                if backend.endpoint() == endpoint {
                    backend.healthy = true;
                }
            }
        }
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_backend(node: &str, addr: &str, port: u16) -> Backend {
        Backend {
            node_id: node.to_string(),
            address: addr.to_string(),
            port,
            healthy: true,
        }
    }

    #[test]
    fn round_robin_cycles() {
        let router = Router::new();
        router.update_service(
            "api",
            vec![
                make_backend("n1", "10.0.0.1", 8080),
                make_backend("n2", "10.0.0.2", 8080),
                make_backend("n3", "10.0.0.3", 8080),
            ],
        );

        let b1 = router.next_backend("api").unwrap();
        let b2 = router.next_backend("api").unwrap();
        let b3 = router.next_backend("api").unwrap();
        let b4 = router.next_backend("api").unwrap(); // Wraps around.

        assert_eq!(b1.endpoint(), "10.0.0.1:8080");
        assert_eq!(b2.endpoint(), "10.0.0.2:8080");
        assert_eq!(b3.endpoint(), "10.0.0.3:8080");
        assert_eq!(b4.endpoint(), "10.0.0.1:8080");
    }

    #[test]
    fn skips_unhealthy_backends() {
        let router = Router::new();
        router.update_service(
            "api",
            vec![
                make_backend("n1", "10.0.0.1", 8080),
                make_backend("n2", "10.0.0.2", 8080),
            ],
        );

        router.mark_unhealthy("api", "10.0.0.1:8080");

        // Should only return the healthy one.
        let b = router.next_backend("api").unwrap();
        assert_eq!(b.endpoint(), "10.0.0.2:8080");
    }

    #[test]
    fn returns_none_for_unknown_service() {
        let router = Router::new();
        assert!(router.next_backend("nonexistent").is_none());
    }

    #[test]
    fn returns_none_when_all_unhealthy() {
        let router = Router::new();
        router.update_service(
            "api",
            vec![make_backend("n1", "10.0.0.1", 8080)],
        );
        router.mark_unhealthy("api", "10.0.0.1:8080");

        assert!(router.next_backend("api").is_none());
    }

    #[test]
    fn remove_service_works() {
        let router = Router::new();
        router.update_service("api", vec![make_backend("n1", "10.0.0.1", 8080)]);
        assert!(router.next_backend("api").is_some());

        router.remove_service("api");
        assert!(router.next_backend("api").is_none());
    }

    #[test]
    fn list_services_returns_all() {
        let router = Router::new();
        router.update_service("api", vec![]);
        router.update_service("web", vec![]);

        let mut services = router.list_services();
        services.sort();
        assert_eq!(services, vec!["api", "web"]);
    }
}
