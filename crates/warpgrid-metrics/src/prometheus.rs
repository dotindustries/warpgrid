//! Prometheus text exposition format.
//!
//! Renders metrics snapshots into the Prometheus text exposition format
//! for scraping by a Prometheus server or compatible agent.

use warpgrid_state::MetricsSnapshot;

/// Render a list of metrics snapshots into Prometheus text format.
///
/// Produces GAUGE and COUNTER metrics with `deployment` labels.
pub fn render_prometheus(snapshots: &[MetricsSnapshot]) -> String {
    let mut out = String::new();

    // Help + type declarations.
    out.push_str("# HELP warpgrid_requests_per_second Current requests per second.\n");
    out.push_str("# TYPE warpgrid_requests_per_second gauge\n");
    for s in snapshots {
        out.push_str(&format!(
            "warpgrid_requests_per_second{{deployment=\"{}\"}} {:.2}\n",
            s.deployment_id, s.rps
        ));
    }

    out.push_str("# HELP warpgrid_latency_p50_ms P50 latency in milliseconds.\n");
    out.push_str("# TYPE warpgrid_latency_p50_ms gauge\n");
    for s in snapshots {
        out.push_str(&format!(
            "warpgrid_latency_p50_ms{{deployment=\"{}\"}} {:.2}\n",
            s.deployment_id, s.latency_p50_ms
        ));
    }

    out.push_str("# HELP warpgrid_latency_p99_ms P99 latency in milliseconds.\n");
    out.push_str("# TYPE warpgrid_latency_p99_ms gauge\n");
    for s in snapshots {
        out.push_str(&format!(
            "warpgrid_latency_p99_ms{{deployment=\"{}\"}} {:.2}\n",
            s.deployment_id, s.latency_p99_ms
        ));
    }

    out.push_str("# HELP warpgrid_error_rate Error rate (0.0-1.0).\n");
    out.push_str("# TYPE warpgrid_error_rate gauge\n");
    for s in snapshots {
        out.push_str(&format!(
            "warpgrid_error_rate{{deployment=\"{}\"}} {:.4}\n",
            s.deployment_id, s.error_rate
        ));
    }

    out.push_str("# HELP warpgrid_memory_bytes Total memory usage in bytes.\n");
    out.push_str("# TYPE warpgrid_memory_bytes gauge\n");
    for s in snapshots {
        out.push_str(&format!(
            "warpgrid_memory_bytes{{deployment=\"{}\"}} {}\n",
            s.deployment_id, s.total_memory_bytes
        ));
    }

    out.push_str("# HELP warpgrid_active_instances Number of active instances.\n");
    out.push_str("# TYPE warpgrid_active_instances gauge\n");
    for s in snapshots {
        out.push_str(&format!(
            "warpgrid_active_instances{{deployment=\"{}\"}} {}\n",
            s.deployment_id, s.active_instances
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_snapshot(deployment_id: &str) -> MetricsSnapshot {
        MetricsSnapshot {
            deployment_id: deployment_id.to_string(),
            epoch: 1000,
            rps: 150.5,
            latency_p50_ms: 5.2,
            latency_p99_ms: 45.8,
            error_rate: 0.012,
            total_memory_bytes: 256_000_000,
            active_instances: 4,
        }
    }

    #[test]
    fn render_empty() {
        let output = render_prometheus(&[]);
        // Should still have type declarations.
        assert!(output.contains("# HELP warpgrid_requests_per_second"));
        assert!(output.contains("# TYPE warpgrid_requests_per_second gauge"));
    }

    #[test]
    fn render_single_deployment() {
        let snapshots = vec![test_snapshot("default/my-api")];
        let output = render_prometheus(&snapshots);

        assert!(output.contains("warpgrid_requests_per_second{deployment=\"default/my-api\"} 150.50"));
        assert!(output.contains("warpgrid_latency_p50_ms{deployment=\"default/my-api\"} 5.20"));
        assert!(output.contains("warpgrid_latency_p99_ms{deployment=\"default/my-api\"} 45.80"));
        assert!(output.contains("warpgrid_error_rate{deployment=\"default/my-api\"} 0.0120"));
        assert!(output.contains("warpgrid_memory_bytes{deployment=\"default/my-api\"} 256000000"));
        assert!(output.contains("warpgrid_active_instances{deployment=\"default/my-api\"} 4"));
    }

    #[test]
    fn render_multiple_deployments() {
        let snapshots = vec![
            test_snapshot("ns1/api"),
            test_snapshot("ns2/worker"),
        ];
        let output = render_prometheus(&snapshots);

        assert!(output.contains("deployment=\"ns1/api\""));
        assert!(output.contains("deployment=\"ns2/worker\""));
    }

    #[test]
    fn render_format_is_prometheus_compatible() {
        let snapshots = vec![test_snapshot("test")];
        let output = render_prometheus(&snapshots);

        // Every non-empty, non-comment line should match: metric_name{labels} value
        for line in output.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            assert!(
                line.contains('{') && line.contains('}'),
                "line should have labels: {line}"
            );
        }
    }
}
