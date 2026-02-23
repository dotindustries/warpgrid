//! Proxy state synchronization — bridges the state store to proxy components.
//!
//! `ProxySync` reads deployments and instances from the state store and
//! rebuilds router backends and DNS records. It provides both full-sync
//! and event-driven update methods.

use tracing::{debug, info};

use warpgrid_state::{DeploymentSpec, InstanceState, InstanceStatus, StateStore};

use crate::dns::DnsResolver;
use crate::router::{Backend, Router};

/// Bridges the state store to the service mesh proxy components.
///
/// On each `sync()` call, it reads all deployments and their running
/// instances from the state store, then rebuilds:
/// - Router backends (for load-balanced request routing)
/// - DNS records (for internal service discovery)
pub struct ProxySync {
    router: Router,
    dns: DnsResolver,
}

impl ProxySync {
    /// Create a new `ProxySync` with the given router and DNS resolver.
    pub fn new(router: Router, dns: DnsResolver) -> Self {
        Self { router, dns }
    }

    /// Access the underlying router.
    pub fn router(&self) -> &Router {
        &self.router
    }

    /// Access the underlying DNS resolver.
    pub fn dns(&self) -> &DnsResolver {
        &self.dns
    }

    /// Full rebuild: sync all deployments from the state store.
    ///
    /// Reads every deployment and its instances, updates router backends
    /// and DNS records. Any services not present in the store are removed.
    pub fn sync(&self, store: &StateStore) -> Result<SyncStats, warpgrid_state::StateError> {
        let deployments = store.list_deployments()?;
        let mut stats = SyncStats::default();

        // Track which services we've seen so we can clean up stale ones.
        let existing_services: Vec<String> = self.router.list_services();
        let mut seen_services = Vec::new();

        for spec in &deployments {
            let service_name = service_key(&spec.namespace, &spec.name);
            seen_services.push(service_name.clone());

            let instances = store.list_instances_for_deployment(&spec.id)?;
            let backends = instances_to_backends(&instances);
            let addresses: Vec<String> = backends.iter().map(|b| b.endpoint()).collect();

            self.router.update_service(&service_name, backends);
            self.dns.upsert(
                &spec.name,
                &spec.namespace,
                addresses,
                60, // 60s TTL
            );

            stats.services_synced += 1;
            stats.backends_total += instances.len() as u32;
        }

        // Remove stale services that no longer exist in the store.
        for service in &existing_services {
            if !seen_services.contains(service) {
                self.router.remove_service(service);
                // Parse namespace/name from service key for DNS removal.
                if let Some((ns, name)) = service.split_once('/') {
                    self.dns.remove(name, ns);
                }
                stats.services_removed += 1;
            }
        }

        info!(
            services = stats.services_synced,
            backends = stats.backends_total,
            removed = stats.services_removed,
            "proxy sync complete"
        );

        Ok(stats)
    }

    /// Event-driven: sync a single deployment after create/update.
    pub fn on_deploy(
        &self,
        spec: &DeploymentSpec,
        instances: &[InstanceState],
    ) {
        let service_name = service_key(&spec.namespace, &spec.name);
        let backends = instances_to_backends(instances);
        let addresses: Vec<String> = backends.iter().map(|b| b.endpoint()).collect();

        self.router.update_service(&service_name, backends);
        self.dns.upsert(&spec.name, &spec.namespace, addresses, 60);

        debug!(
            service = %service_name,
            instances = instances.len(),
            "synced deployment to proxy"
        );
    }

    /// Event-driven: remove a deployment from the proxy.
    pub fn on_undeploy(&self, namespace: &str, name: &str) {
        let service_name = service_key(namespace, name);
        self.router.remove_service(&service_name);
        self.dns.remove(name, namespace);

        debug!(
            service = %service_name,
            "removed deployment from proxy"
        );
    }
}

/// Sync statistics.
#[derive(Debug, Default)]
pub struct SyncStats {
    pub services_synced: u32,
    pub backends_total: u32,
    pub services_removed: u32,
}

/// Build a service key from namespace and name.
fn service_key(namespace: &str, name: &str) -> String {
    format!("{namespace}/{name}")
}

/// Convert instance states to router backends.
///
/// Only instances in `Running` status are included. Unhealthy instances
/// are included but marked as unhealthy so the router can skip them.
fn instances_to_backends(instances: &[InstanceState]) -> Vec<Backend> {
    instances
        .iter()
        .filter(|i| i.status == InstanceStatus::Running || i.status == InstanceStatus::Unhealthy)
        .map(|i| Backend {
            node_id: i.node_id.clone(),
            address: i.node_id.clone(), // Node ID used as address placeholder.
            port: 0,                    // Port resolved at request time.
            healthy: i.status == InstanceStatus::Running,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use warpgrid_state::*;

    fn test_store() -> StateStore {
        StateStore::open_in_memory().unwrap()
    }

    fn make_spec(ns: &str, name: &str) -> DeploymentSpec {
        DeploymentSpec {
            id: format!("{ns}/{name}"),
            namespace: ns.to_string(),
            name: name.to_string(),
            source: "file://test.wasm".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: InstanceConstraints { min: 1, max: 5 },
            resources: ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: ShimsEnabled::default(),
            env: HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        }
    }

    fn make_instance(id: &str, deployment: &str, node: &str, status: InstanceStatus) -> InstanceState {
        InstanceState {
            id: id.to_string(),
            deployment_id: deployment.to_string(),
            node_id: node.to_string(),
            status,
            health: HealthStatus::Unknown,
            restart_count: 0,
            memory_bytes: 0,
            started_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn sync_registers_backends_and_dns() {
        let store = test_store();
        let spec = make_spec("prod", "api");
        store.put_deployment(&spec).unwrap();

        let inst = make_instance("i1", "prod/api", "node-1", InstanceStatus::Running);
        store.put_instance(&inst).unwrap();

        let sync = ProxySync::new(Router::new(), DnsResolver::default());
        let stats = sync.sync(&store).unwrap();

        assert_eq!(stats.services_synced, 1);
        assert_eq!(stats.backends_total, 1);

        // Router has the service.
        let backends = sync.router().get_backends("prod/api");
        assert_eq!(backends.len(), 1);
        assert_eq!(backends[0].node_id, "node-1");

        // DNS has the record.
        let record = sync.dns().resolve_service("api", "prod").unwrap();
        assert_eq!(record.addresses.len(), 1);
    }

    #[test]
    fn sync_removes_stale_services() {
        let store = test_store();
        let sync = ProxySync::new(Router::new(), DnsResolver::default());

        // Pre-populate with a service.
        sync.router().update_service("old/stale", vec![]);

        // Sync with empty store — should remove the stale service.
        let stats = sync.sync(&store).unwrap();
        assert_eq!(stats.services_removed, 1);
        assert!(sync.router().list_services().is_empty());
    }

    #[test]
    fn on_deploy_updates_router_and_dns() {
        let spec = make_spec("prod", "web");
        let instances = vec![
            make_instance("i1", "prod/web", "node-1", InstanceStatus::Running),
            make_instance("i2", "prod/web", "node-2", InstanceStatus::Running),
        ];

        let sync = ProxySync::new(Router::new(), DnsResolver::default());
        sync.on_deploy(&spec, &instances);

        let backends = sync.router().get_backends("prod/web");
        assert_eq!(backends.len(), 2);

        let record = sync.dns().resolve_service("web", "prod").unwrap();
        assert_eq!(record.addresses.len(), 2);
    }

    #[test]
    fn on_undeploy_removes_service() {
        let spec = make_spec("prod", "api");
        let instances = vec![
            make_instance("i1", "prod/api", "node-1", InstanceStatus::Running),
        ];

        let sync = ProxySync::new(Router::new(), DnsResolver::default());
        sync.on_deploy(&spec, &instances);
        assert!(!sync.router().list_services().is_empty());

        sync.on_undeploy("prod", "api");
        assert!(sync.router().list_services().is_empty());
        assert!(sync.dns().resolve_service("api", "prod").is_none());
    }

    #[test]
    fn filters_non_running_instances() {
        let instances = vec![
            make_instance("i1", "d/a", "n1", InstanceStatus::Running),
            make_instance("i2", "d/a", "n2", InstanceStatus::Starting),
            make_instance("i3", "d/a", "n3", InstanceStatus::Stopped),
            make_instance("i4", "d/a", "n4", InstanceStatus::Unhealthy),
        ];

        let backends = instances_to_backends(&instances);
        // Running + Unhealthy included, Starting + Stopped excluded.
        assert_eq!(backends.len(), 2);
        assert!(backends[0].healthy);  // Running
        assert!(!backends[1].healthy); // Unhealthy
    }

    #[test]
    fn empty_deployment_registers_empty_backends() {
        let store = test_store();
        let spec = make_spec("prod", "api");
        store.put_deployment(&spec).unwrap();
        // No instances.

        let sync = ProxySync::new(Router::new(), DnsResolver::default());
        let stats = sync.sync(&store).unwrap();

        assert_eq!(stats.services_synced, 1);
        assert_eq!(stats.backends_total, 0);

        let backends = sync.router().get_backends("prod/api");
        assert!(backends.is_empty());
    }

    #[test]
    fn multiple_deployments_sync() {
        let store = test_store();

        let spec1 = make_spec("prod", "api");
        let spec2 = make_spec("prod", "web");
        store.put_deployment(&spec1).unwrap();
        store.put_deployment(&spec2).unwrap();

        let i1 = make_instance("i1", "prod/api", "n1", InstanceStatus::Running);
        let i2 = make_instance("i2", "prod/web", "n1", InstanceStatus::Running);
        let i3 = make_instance("i3", "prod/web", "n2", InstanceStatus::Running);
        store.put_instance(&i1).unwrap();
        store.put_instance(&i2).unwrap();
        store.put_instance(&i3).unwrap();

        let sync = ProxySync::new(Router::new(), DnsResolver::default());
        let stats = sync.sync(&store).unwrap();

        assert_eq!(stats.services_synced, 2);
        assert_eq!(stats.backends_total, 3);

        assert_eq!(sync.router().get_backends("prod/api").len(), 1);
        assert_eq!(sync.router().get_backends("prod/web").len(), 2);
    }
}
