//! Integration tests for the warpgrid-proxy crate.
//!
//! Tests cover full state sync, stale service removal, router health
//! filtering, DNS namespace isolation, TLS wildcard vs exact match
//! precedence, and event-driven deploy/undeploy updates.

use std::collections::HashMap;

use warpgrid_proxy::{Backend, DnsResolver, ProxySync, Router, TlsCert, TlsTerminator};
use warpgrid_state::*;

// ── Helpers ──────────────────────────────────────────────────────────

fn in_memory_state() -> StateStore {
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

fn make_backend(node: &str, addr: &str, port: u16, healthy: bool) -> Backend {
    Backend {
        node_id: node.to_string(),
        address: addr.to_string(),
        port,
        healthy,
    }
}

fn make_cert(name: &str, is_default: bool) -> TlsCert {
    TlsCert {
        server_name: name.to_string(),
        cert_pem: format!("cert-{name}"),
        key_pem: format!("key-{name}"),
        is_default,
    }
}

// ── 1. Full sync from state store ────────────────────────────────────

#[test]
fn full_sync_registers_all_deployments_as_services() {
    let store = in_memory_state();

    let spec1 = make_spec("prod", "api");
    let spec2 = make_spec("prod", "web");
    store.put_deployment(&spec1).unwrap();
    store.put_deployment(&spec2).unwrap();

    // Add running instances for both deployments.
    store
        .put_instance(&make_instance(
            "i1",
            "prod/api",
            "node-1",
            InstanceStatus::Running,
        ))
        .unwrap();
    store
        .put_instance(&make_instance(
            "i2",
            "prod/api",
            "node-2",
            InstanceStatus::Running,
        ))
        .unwrap();
    store
        .put_instance(&make_instance(
            "i3",
            "prod/web",
            "node-1",
            InstanceStatus::Running,
        ))
        .unwrap();

    let sync = ProxySync::new(Router::new(), DnsResolver::default());
    let stats = sync.sync(&store).unwrap();

    assert_eq!(stats.services_synced, 2);
    assert_eq!(stats.backends_total, 3);
    assert_eq!(stats.services_removed, 0);

    // Router has both services.
    let api_backends = sync.router().get_backends("prod/api");
    assert_eq!(api_backends.len(), 2);

    let web_backends = sync.router().get_backends("prod/web");
    assert_eq!(web_backends.len(), 1);

    // DNS has records for both.
    let api_dns = sync.dns().resolve_service("api", "prod").unwrap();
    assert_eq!(api_dns.addresses.len(), 2);

    let web_dns = sync.dns().resolve_service("web", "prod").unwrap();
    assert_eq!(web_dns.addresses.len(), 1);
}

#[test]
fn full_sync_with_empty_store_produces_no_services() {
    let store = in_memory_state();
    let sync = ProxySync::new(Router::new(), DnsResolver::default());
    let stats = sync.sync(&store).unwrap();

    assert_eq!(stats.services_synced, 0);
    assert_eq!(stats.backends_total, 0);
    assert!(sync.router().list_services().is_empty());
}

#[test]
fn full_sync_deployment_with_no_instances_registers_empty_backend_list() {
    let store = in_memory_state();
    store.put_deployment(&make_spec("prod", "api")).unwrap();

    let sync = ProxySync::new(Router::new(), DnsResolver::default());
    let stats = sync.sync(&store).unwrap();

    assert_eq!(stats.services_synced, 1);
    assert_eq!(stats.backends_total, 0);

    let backends = sync.router().get_backends("prod/api");
    assert!(backends.is_empty());
}

// ── 2. Sync removes stale services ──────────────────────────────────

#[test]
fn sync_removes_services_not_in_state_store() {
    let store = in_memory_state();
    let sync = ProxySync::new(Router::new(), DnsResolver::default());

    // Pre-populate the router with a service that does not exist in the store.
    sync.router().update_service(
        "stale/old-svc",
        vec![make_backend("n1", "10.0.0.1", 8080, true)],
    );

    // Sync with empty store should remove the stale service.
    let stats = sync.sync(&store).unwrap();
    assert_eq!(stats.services_removed, 1);
    assert!(sync.router().list_services().is_empty());
}

#[test]
fn sync_removes_stale_but_keeps_valid_services() {
    let store = in_memory_state();
    let sync = ProxySync::new(Router::new(), DnsResolver::default());

    // Pre-populate router with two services.
    sync.router()
        .update_service("prod/api", vec![make_backend("n1", "10.0.0.1", 8080, true)]);
    sync.router().update_service(
        "stale/gone",
        vec![make_backend("n2", "10.0.0.2", 8080, true)],
    );

    // Only put prod/api in the state store.
    store.put_deployment(&make_spec("prod", "api")).unwrap();
    store
        .put_instance(&make_instance(
            "i1",
            "prod/api",
            "node-1",
            InstanceStatus::Running,
        ))
        .unwrap();

    let stats = sync.sync(&store).unwrap();
    assert_eq!(stats.services_synced, 1);
    assert_eq!(stats.services_removed, 1);

    // prod/api is present, stale/gone is removed.
    let services = sync.router().list_services();
    assert_eq!(services.len(), 1);
    assert!(services.contains(&"prod/api".to_string()));
}

// ── 3. Router skips unhealthy backends ───────────────────────────────

#[test]
fn router_skips_unhealthy_backends_in_round_robin() {
    let router = Router::new();

    router.update_service(
        "api",
        vec![
            make_backend("n1", "10.0.0.1", 8080, true),
            make_backend("n2", "10.0.0.2", 8080, true),
            make_backend("n3", "10.0.0.3", 8080, true),
        ],
    );

    // Mark the second backend as unhealthy.
    router.mark_unhealthy("api", "10.0.0.2:8080");

    // Collect several selections — none should be the unhealthy backend.
    let mut selected_endpoints = Vec::new();
    for _ in 0..10 {
        let b = router.next_backend("api").unwrap();
        selected_endpoints.push(b.endpoint());
    }

    assert!(
        !selected_endpoints.contains(&"10.0.0.2:8080".to_string()),
        "unhealthy backend should never be selected"
    );
    // Should only cycle between the two healthy ones.
    assert!(selected_endpoints.contains(&"10.0.0.1:8080".to_string()));
    assert!(selected_endpoints.contains(&"10.0.0.3:8080".to_string()));
}

#[test]
fn router_returns_none_when_all_backends_unhealthy() {
    let router = Router::new();
    router.update_service(
        "api",
        vec![
            make_backend("n1", "10.0.0.1", 8080, true),
            make_backend("n2", "10.0.0.2", 8080, true),
        ],
    );

    router.mark_unhealthy("api", "10.0.0.1:8080");
    router.mark_unhealthy("api", "10.0.0.2:8080");

    assert!(router.next_backend("api").is_none());
}

#[test]
fn router_re_enables_backend_after_mark_healthy() {
    let router = Router::new();
    router.update_service(
        "api",
        vec![
            make_backend("n1", "10.0.0.1", 8080, true),
            make_backend("n2", "10.0.0.2", 8080, true),
        ],
    );

    // Mark unhealthy then re-enable.
    router.mark_unhealthy("api", "10.0.0.1:8080");
    assert_eq!(
        router.next_backend("api").unwrap().endpoint(),
        "10.0.0.2:8080"
    );

    router.mark_healthy("api", "10.0.0.1:8080");

    // Both should now be selectable.
    let mut seen = std::collections::HashSet::new();
    for _ in 0..10 {
        seen.insert(router.next_backend("api").unwrap().endpoint());
    }
    assert!(seen.contains("10.0.0.1:8080"));
    assert!(seen.contains("10.0.0.2:8080"));
}

// ── 4. DNS resolver namespace isolation ──────────────────────────────

#[test]
fn dns_isolates_same_service_name_across_namespaces() {
    let dns = DnsResolver::new("warpgrid");

    dns.upsert(
        "api",
        "prod",
        vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()],
        60,
    );
    dns.upsert("api", "staging", vec!["10.1.0.1".to_string()], 30);

    // Resolving "api" in "prod" should return prod addresses only.
    let prod = dns.resolve_service("api", "prod").unwrap();
    assert_eq!(prod.fqdn, "api.prod.svc.warpgrid");
    assert_eq!(prod.addresses, vec!["10.0.0.1", "10.0.0.2"]);
    assert_eq!(prod.ttl, 60);

    // Resolving "api" in "staging" should return staging addresses only.
    let staging = dns.resolve_service("api", "staging").unwrap();
    assert_eq!(staging.fqdn, "api.staging.svc.warpgrid");
    assert_eq!(staging.addresses, vec!["10.1.0.1"]);
    assert_eq!(staging.ttl, 30);
}

#[test]
fn dns_resolve_by_fqdn_directly() {
    let dns = DnsResolver::new("warpgrid");
    dns.upsert("web", "default", vec!["10.0.0.5".to_string()], 120);

    let record = dns.resolve("web.default.svc.warpgrid").unwrap();
    assert_eq!(record.addresses, vec!["10.0.0.5"]);
}

#[test]
fn dns_returns_none_for_unknown_service() {
    let dns = DnsResolver::new("warpgrid");
    assert!(dns.resolve_service("nonexistent", "default").is_none());
}

#[test]
fn dns_remove_cleans_up_record() {
    let dns = DnsResolver::new("warpgrid");
    dns.upsert("api", "prod", vec!["10.0.0.1".to_string()], 60);
    dns.remove("api", "prod");
    assert!(dns.resolve_service("api", "prod").is_none());
}

#[test]
fn dns_upsert_overwrites_existing_record() {
    let dns = DnsResolver::new("warpgrid");
    dns.upsert("api", "prod", vec!["10.0.0.1".to_string()], 60);
    dns.upsert(
        "api",
        "prod",
        vec!["10.0.0.1".to_string(), "10.0.0.2".to_string()],
        30,
    );

    let record = dns.resolve_service("api", "prod").unwrap();
    assert_eq!(record.addresses.len(), 2);
    assert_eq!(record.ttl, 30);
}

// ── 5. TLS wildcard vs exact match precedence ────────────────────────

#[test]
fn tls_exact_match_takes_precedence_over_wildcard() {
    let mut term = TlsTerminator::new();

    term.upsert_cert(make_cert("*.example.com", false));
    term.upsert_cert(make_cert("api.example.com", false));

    // Exact match should be returned for api.example.com.
    let cert = term.resolve("api.example.com").unwrap();
    assert_eq!(cert.server_name, "api.example.com");
}

#[test]
fn tls_wildcard_matches_single_subdomain_level() {
    let mut term = TlsTerminator::new();
    term.upsert_cert(make_cert("*.example.com", false));

    let cert = term.resolve("web.example.com").unwrap();
    assert_eq!(cert.server_name, "*.example.com");
}

#[test]
fn tls_wildcard_does_not_match_multi_level_subdomain() {
    let mut term = TlsTerminator::new();
    term.upsert_cert(make_cert("*.example.com", false));

    // *.example.com should NOT match sub.api.example.com.
    assert!(term.resolve("sub.api.example.com").is_none());
}

#[test]
fn tls_falls_back_to_default_cert() {
    let mut term = TlsTerminator::new();
    term.upsert_cert(make_cert("default.local", true));
    term.upsert_cert(make_cert("api.specific.com", false));

    // Unknown hostname should fall back to default.
    let cert = term.resolve("unknown.host.com").unwrap();
    assert_eq!(cert.server_name, "default.local");
}

#[test]
fn tls_returns_none_when_no_certs_registered() {
    let term = TlsTerminator::new();
    assert!(term.resolve("anything.com").is_none());
}

#[test]
fn tls_remove_cert_clears_entry_and_default() {
    let mut term = TlsTerminator::new();
    term.upsert_cert(make_cert("api.example.com", true));

    term.remove_cert("api.example.com");
    assert!(term.resolve("api.example.com").is_none());
    // Default was cleared too, so unknown hosts also return None.
    assert!(term.resolve("other.host").is_none());
}

#[test]
fn tls_key_not_serialized() {
    let cert = make_cert("test.com", false);
    let json = serde_json::to_string(&cert).unwrap();
    assert!(
        !json.contains("key-test.com"),
        "key_pem should not appear in serialized output"
    );
}

// ── 6. Event-driven deploy/undeploy updates ──────────────────────────

#[test]
fn on_deploy_registers_router_backends_and_dns() {
    let sync = ProxySync::new(Router::new(), DnsResolver::default());

    let spec = make_spec("prod", "api");
    let instances = vec![
        make_instance("i1", "prod/api", "node-1", InstanceStatus::Running),
        make_instance("i2", "prod/api", "node-2", InstanceStatus::Running),
    ];

    sync.on_deploy(&spec, &instances);

    // Router should have the service with 2 backends.
    let backends = sync.router().get_backends("prod/api");
    assert_eq!(backends.len(), 2);

    // DNS should have the record.
    let record = sync.dns().resolve_service("api", "prod").unwrap();
    assert_eq!(record.addresses.len(), 2);
}

#[test]
fn on_deploy_filters_non_running_instances() {
    let sync = ProxySync::new(Router::new(), DnsResolver::default());

    let spec = make_spec("prod", "api");
    let instances = vec![
        make_instance("i1", "prod/api", "node-1", InstanceStatus::Running),
        make_instance("i2", "prod/api", "node-2", InstanceStatus::Starting),
        make_instance("i3", "prod/api", "node-3", InstanceStatus::Stopped),
        make_instance("i4", "prod/api", "node-4", InstanceStatus::Unhealthy),
    ];

    sync.on_deploy(&spec, &instances);

    // Only Running + Unhealthy should be backends.
    let backends = sync.router().get_backends("prod/api");
    assert_eq!(backends.len(), 2);

    // Running backend should be healthy; Unhealthy should be marked unhealthy.
    let healthy_count = backends.iter().filter(|b| b.healthy).count();
    let unhealthy_count = backends.iter().filter(|b| !b.healthy).count();
    assert_eq!(healthy_count, 1);
    assert_eq!(unhealthy_count, 1);
}

#[test]
fn on_undeploy_removes_router_and_dns_entries() {
    let sync = ProxySync::new(Router::new(), DnsResolver::default());

    // Deploy first.
    let spec = make_spec("prod", "api");
    let instances = vec![make_instance(
        "i1",
        "prod/api",
        "node-1",
        InstanceStatus::Running,
    )];
    sync.on_deploy(&spec, &instances);

    assert!(!sync.router().list_services().is_empty());
    assert!(sync.dns().resolve_service("api", "prod").is_some());

    // Undeploy should clean up both router and DNS.
    sync.on_undeploy("prod", "api");

    assert!(sync.router().list_services().is_empty());
    assert!(sync.dns().resolve_service("api", "prod").is_none());
}

#[test]
fn on_deploy_updates_existing_service_backends() {
    let sync = ProxySync::new(Router::new(), DnsResolver::default());

    let spec = make_spec("prod", "api");

    // Initial deploy with 1 instance.
    let instances_v1 = vec![make_instance(
        "i1",
        "prod/api",
        "node-1",
        InstanceStatus::Running,
    )];
    sync.on_deploy(&spec, &instances_v1);
    assert_eq!(sync.router().get_backends("prod/api").len(), 1);

    // Updated deploy with 3 instances (scale-up).
    let instances_v2 = vec![
        make_instance("i1", "prod/api", "node-1", InstanceStatus::Running),
        make_instance("i2", "prod/api", "node-2", InstanceStatus::Running),
        make_instance("i3", "prod/api", "node-3", InstanceStatus::Running),
    ];
    sync.on_deploy(&spec, &instances_v2);
    assert_eq!(sync.router().get_backends("prod/api").len(), 3);

    // DNS should also reflect the updated address count.
    let record = sync.dns().resolve_service("api", "prod").unwrap();
    assert_eq!(record.addresses.len(), 3);
}
