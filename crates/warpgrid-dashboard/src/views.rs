//! View types for dashboard template rendering.
//!
//! These types are purpose-built for Askama templates: they carry
//! pre-formatted strings and computed fields so templates stay simple.

use std::collections::HashMap;

use warpgrid_rollout::{Rollout, RolloutPhase, RolloutStrategy};
use warpgrid_state::{
    DeploymentSpec, HealthStatus, InstanceState, InstanceStatus, MetricsSnapshot, NodeInfo,
    TriggerConfig,
};

// ── Cluster Summary ─────────────────────────────────────────────

pub struct ClusterSummary {
    pub deployment_count: usize,
    pub namespace_counts: Vec<(String, usize)>,
    pub instances: InstanceCounts,
    pub node_count: usize,
    pub nodes_ready: usize,
    pub active_rollouts: usize,
    pub cluster_memory: ResourceBar,
    pub cluster_cpu: ResourceBar,
}

pub struct InstanceCounts {
    pub running: usize,
    pub unhealthy: usize,
    pub stopped: usize,
    pub total: usize,
}

pub struct ResourceBar {
    pub used: u64,
    pub total: u64,
    pub percent: f64,
    pub percent_display: String,
    pub percent_int: String,
    pub used_display: String,
    pub total_display: String,
}

impl ResourceBar {
    pub fn memory(used: u64, total: u64) -> Self {
        let percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        Self {
            used,
            total,
            percent,
            percent_display: format!("{:.1}", percent),
            percent_int: format!("{:.0}", percent),
            used_display: format_bytes(used),
            total_display: format_bytes(total),
        }
    }

    pub fn cpu(used: u32, total: u32) -> Self {
        let percent = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        Self {
            used: used as u64,
            total: total as u64,
            percent,
            percent_display: format!("{:.1}", percent),
            percent_int: format!("{:.0}", percent),
            used_display: used.to_string(),
            total_display: total.to_string(),
        }
    }

    pub fn bar_color(&self) -> &'static str {
        if self.percent > 90.0 {
            "bg-grid-danger"
        } else if self.percent > 70.0 {
            "bg-grid-warn"
        } else {
            "bg-grid-accent"
        }
    }
}

// ── Deployment View ─────────────────────────────────────────────

pub struct DeploymentView {
    pub id: String,
    pub name: String,
    pub namespace: String,
    pub source: String,
    pub trigger_display: String,
    pub trigger_icon: &'static str,
    pub instances_running: usize,
    pub instances_max: u32,
    pub instances_min: u32,
    pub instance_percent: f64,
    pub instance_percent_display: String,
    pub health_dots: Vec<HealthDot>,
    pub latest_rps: Option<f64>,
    pub latest_rps_display: Option<String>,
    pub error_rate: Option<f64>,
    pub error_rate_display: Option<String>,
    pub error_rate_color: &'static str,
    pub shims: ShimIcons,
    pub memory_display: String,
    pub cpu_weight: u32,
    pub created_display: String,
    pub updated_display: String,
    pub scaling: Option<ScalingView>,
    pub health_config: Option<HealthConfigView>,
    pub env_vars: Vec<(String, String)>,
}

pub struct HealthDot {
    pub color: &'static str,
    pub title: String,
}

pub struct ShimIcons {
    pub timezone: bool,
    pub dev_urandom: bool,
    pub dns: bool,
    pub signals: bool,
    pub database_proxy: bool,
}

pub struct ScalingView {
    pub metric: String,
    pub target_value: f64,
    pub scale_up_window: String,
    pub scale_down_window: String,
}

pub struct HealthConfigView {
    pub endpoint: String,
    pub interval: String,
    pub timeout: String,
    pub unhealthy_threshold: u32,
}

impl DeploymentView {
    pub fn from_spec(
        spec: &DeploymentSpec,
        instances: &[InstanceState],
        latest_metrics: Option<&MetricsSnapshot>,
    ) -> Self {
        let running = instances
            .iter()
            .filter(|i| i.status == InstanceStatus::Running)
            .count();

        let instance_percent = if spec.instances.max > 0 {
            (running as f64 / spec.instances.max as f64) * 100.0
        } else {
            0.0
        };

        let health_dots: Vec<HealthDot> = instances
            .iter()
            .map(|i| HealthDot {
                color: instance_status_color(i.status, i.health),
                title: format!("{} — {:?}/{:?}", i.id, i.status, i.health),
            })
            .collect();

        let (trigger_display, trigger_icon) = format_trigger(&spec.trigger);

        let error_rate = latest_metrics.map(|m| m.error_rate * 100.0);
        let error_rate_color = match error_rate {
            Some(r) if r > 5.0 => "text-rose-400",
            Some(r) if r > 1.0 => "text-amber-400",
            _ => "text-emerald-400",
        };

        let scaling = spec.scaling.as_ref().map(|s| ScalingView {
            metric: s.metric.clone(),
            target_value: s.target_value,
            scale_up_window: s.scale_up_window.clone(),
            scale_down_window: s.scale_down_window.clone(),
        });

        let health_config = spec.health.as_ref().map(|h| HealthConfigView {
            endpoint: h.endpoint.clone(),
            interval: h.interval.clone(),
            timeout: h.timeout.clone(),
            unhealthy_threshold: h.unhealthy_threshold,
        });

        let mut env_vars: Vec<(String, String)> = spec
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env_vars.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            id: spec.id.clone(),
            name: spec.name.clone(),
            namespace: spec.namespace.clone(),
            source: spec.source.clone(),
            trigger_display,
            trigger_icon,
            instances_running: running,
            instances_max: spec.instances.max,
            instances_min: spec.instances.min,
            instance_percent,
            instance_percent_display: format!("{:.1}", instance_percent),
            health_dots,
            latest_rps: latest_metrics.map(|m| m.rps),
            latest_rps_display: latest_metrics.map(|m| format!("{:.1}", m.rps)),
            error_rate,
            error_rate_display: error_rate.map(|r| format!("{:.2}", r)),
            error_rate_color,
            shims: ShimIcons {
                timezone: spec.shims.timezone,
                dev_urandom: spec.shims.dev_urandom,
                dns: spec.shims.dns,
                signals: spec.shims.signals,
                database_proxy: spec.shims.database_proxy,
            },
            memory_display: format_bytes(spec.resources.memory_bytes),
            cpu_weight: spec.resources.cpu_weight,
            created_display: format_timestamp(spec.created_at),
            updated_display: format_relative_time(spec.updated_at),
            scaling,
            health_config,
            env_vars,
        }
    }
}

// ── Instance View ───────────────────────────────────────────────

pub struct InstanceView {
    pub id: String,
    pub deployment_id: String,
    pub node_id: String,
    pub status: String,
    pub status_color: &'static str,
    pub health: String,
    pub health_color: &'static str,
    pub restart_count: u32,
    pub memory_display: String,
    pub memory_bytes: u64,
    pub uptime_display: String,
}

impl InstanceView {
    pub fn from_state(state: &InstanceState) -> Self {
        Self {
            id: state.id.clone(),
            deployment_id: state.deployment_id.clone(),
            node_id: state.node_id.clone(),
            status: format!("{:?}", state.status),
            status_color: status_color_for_instance(state.status),
            health: format!("{:?}", state.health),
            health_color: health_color(state.health),
            restart_count: state.restart_count,
            memory_display: format_bytes(state.memory_bytes),
            memory_bytes: state.memory_bytes,
            uptime_display: format_relative_time(state.started_at),
        }
    }
}

// ── Node View ───────────────────────────────────────────────────

pub struct NodeView {
    pub id: String,
    pub address: String,
    pub port: u16,
    pub status: &'static str,
    pub status_color: &'static str,
    pub heartbeat_display: String,
    pub heartbeat_color: &'static str,
    pub memory_bar: ResourceBar,
    pub cpu_bar: ResourceBar,
    pub labels: Vec<(String, String)>,
    pub instance_count: usize,
}

impl NodeView {
    pub fn from_node(node: &NodeInfo, instance_count: usize) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let heartbeat_age = now.saturating_sub(node.last_heartbeat);

        let (status, status_color) = if heartbeat_age > 60 {
            ("Dead", "text-rose-400")
        } else if heartbeat_age > 30 {
            ("Draining", "text-amber-400")
        } else {
            ("Ready", "text-emerald-400")
        };

        let heartbeat_color = if heartbeat_age > 60 {
            "text-rose-400"
        } else if heartbeat_age > 30 {
            "text-amber-400"
        } else {
            "text-slate-400"
        };

        let mut labels: Vec<(String, String)> = node
            .labels
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        labels.sort_by(|a, b| a.0.cmp(&b.0));

        Self {
            id: node.id.clone(),
            address: node.address.clone(),
            port: node.port,
            status,
            status_color,
            heartbeat_display: format_relative_time(node.last_heartbeat),
            heartbeat_color,
            memory_bar: ResourceBar::memory(node.used_memory_bytes, node.capacity_memory_bytes),
            cpu_bar: ResourceBar::cpu(node.used_cpu_weight, node.capacity_cpu_weight),
            labels,
            instance_count,
        }
    }
}

// ── Rollout View ────────────────────────────────────────────────

#[derive(Clone)]
pub struct RolloutView {
    pub deployment_id: String,
    pub strategy_display: String,
    pub strategy_badge_color: &'static str,
    pub phase_display: String,
    pub phase_color: &'static str,
    pub old_version: String,
    pub new_version: String,
    pub target_instances: u32,
    pub progress_percent: f64,
    pub progress_percent_display: String,
    pub progress_text: String,
    pub is_active: bool,
    pub can_pause: bool,
    pub can_resume: bool,
}

impl RolloutView {
    pub fn from_rollout(r: &Rollout) -> Self {
        let (strategy_display, strategy_badge_color) = match &r.strategy {
            RolloutStrategy::Rolling(_) => ("Rolling", "bg-sky-500/20 text-sky-400"),
            RolloutStrategy::Canary(_) => ("Canary", "bg-amber-500/20 text-amber-400"),
            RolloutStrategy::BlueGreen => ("Blue-Green", "bg-emerald-500/20 text-emerald-400"),
        };

        let (phase_display, phase_color, progress_percent, progress_text) = match &r.phase {
            RolloutPhase::Pending => (
                "Pending".to_string(),
                "text-slate-400",
                0.0,
                "Not started".to_string(),
            ),
            RolloutPhase::RollingBatch { current, total } => (
                format!("Rolling {current}/{total}"),
                "text-sky-400",
                (*current as f64 / *total as f64) * 100.0,
                format!("Batch {current}/{total}"),
            ),
            RolloutPhase::CanaryObserving => (
                "Canary Observing".to_string(),
                "text-amber-400",
                25.0,
                "Observing canary".to_string(),
            ),
            RolloutPhase::CanaryPromoting => (
                "Canary Promoting".to_string(),
                "text-emerald-400",
                75.0,
                "Promoting canary".to_string(),
            ),
            RolloutPhase::HealthGate => (
                "Health Gate".to_string(),
                "text-sky-400",
                50.0,
                "Checking health".to_string(),
            ),
            RolloutPhase::Paused => (
                "Paused".to_string(),
                "text-amber-400",
                50.0,
                "Paused by operator".to_string(),
            ),
            RolloutPhase::Completed => (
                "Completed".to_string(),
                "text-emerald-400",
                100.0,
                "Done".to_string(),
            ),
            RolloutPhase::RolledBack { reason } => (
                "Rolled Back".to_string(),
                "text-rose-400",
                100.0,
                reason.clone(),
            ),
        };

        let is_active = !matches!(
            r.phase,
            RolloutPhase::Completed | RolloutPhase::RolledBack { .. }
        );

        Self {
            deployment_id: r.deployment_id.clone(),
            strategy_display: strategy_display.to_string(),
            strategy_badge_color,
            phase_display,
            phase_color,
            old_version: r.old_version.clone(),
            new_version: r.new_version.clone(),
            target_instances: r.target_instances,
            progress_percent,
            progress_percent_display: format!("{:.1}", progress_percent),
            progress_text,
            is_active,
            can_pause: is_active && r.phase != RolloutPhase::Paused,
            can_resume: r.phase == RolloutPhase::Paused,
        }
    }
}

// ── Metrics View ────────────────────────────────────────────────

pub struct MetricsRow {
    pub epoch_display: String,
    pub rps: f64,
    pub rps_display: String,
    pub latency_p50: f64,
    pub latency_p50_display: String,
    pub latency_p99: f64,
    pub latency_p99_display: String,
    pub error_rate_pct: f64,
    pub error_rate_pct_display: String,
    pub error_rate_color: &'static str,
    pub memory_display: String,
    pub active_instances: u32,
    pub rps_bar_width: f64,
    pub rps_bar_width_display: String,
    pub latency_bar_width: f64,
    pub latency_bar_width_display: String,
}

impl MetricsRow {
    pub fn from_snapshot(snap: &MetricsSnapshot, max_rps: f64, max_latency: f64) -> Self {
        let error_pct = snap.error_rate * 100.0;
        let rps_bar_width = if max_rps > 0.0 {
            (snap.rps / max_rps) * 100.0
        } else {
            0.0
        };
        let latency_bar_width = if max_latency > 0.0 {
            (snap.latency_p99_ms / max_latency) * 100.0
        } else {
            0.0
        };
        Self {
            epoch_display: format_timestamp(snap.epoch),
            rps: snap.rps,
            rps_display: format!("{:.1}", snap.rps),
            latency_p50: snap.latency_p50_ms,
            latency_p50_display: format!("{:.1}", snap.latency_p50_ms),
            latency_p99: snap.latency_p99_ms,
            latency_p99_display: format!("{:.1}", snap.latency_p99_ms),
            error_rate_pct: error_pct,
            error_rate_pct_display: format!("{:.2}", error_pct),
            error_rate_color: if error_pct > 5.0 {
                "text-rose-400"
            } else if error_pct > 1.0 {
                "text-amber-400"
            } else {
                "text-emerald-400"
            },
            memory_display: format_bytes(snap.total_memory_bytes),
            active_instances: snap.active_instances,
            rps_bar_width,
            rps_bar_width_display: format!("{:.1}", rps_bar_width),
            latency_bar_width,
            latency_bar_width_display: format!("{:.1}", latency_bar_width),
        }
    }
}

pub fn build_metrics_rows(snapshots: &[MetricsSnapshot]) -> Vec<MetricsRow> {
    let max_rps = snapshots
        .iter()
        .map(|s| s.rps)
        .fold(0.0_f64, f64::max);
    let max_latency = snapshots
        .iter()
        .map(|s| s.latency_p99_ms)
        .fold(0.0_f64, f64::max);

    snapshots
        .iter()
        .map(|s| MetricsRow::from_snapshot(s, max_rps, max_latency))
        .collect()
}

// ── Alert View ──────────────────────────────────────────────────

pub struct AlertView {
    pub severity: &'static str,
    pub severity_color: &'static str,
    pub deployment: String,
    pub message: String,
}

// ── Format Helpers ──────────────────────────────────────────────

pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_relative_time(timestamp_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if timestamp_secs == 0 {
        return "never".to_string();
    }

    let delta = now.saturating_sub(timestamp_secs);

    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

pub fn format_timestamp(timestamp_secs: u64) -> String {
    chrono::DateTime::from_timestamp(timestamp_secs as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn status_color_for_instance(status: InstanceStatus) -> &'static str {
    match status {
        InstanceStatus::Running => "text-emerald-400",
        InstanceStatus::Starting => "text-sky-400",
        InstanceStatus::Unhealthy => "text-rose-400",
        InstanceStatus::Stopping => "text-amber-400",
        InstanceStatus::Stopped => "text-slate-500",
    }
}

pub fn health_color(health: HealthStatus) -> &'static str {
    match health {
        HealthStatus::Healthy => "text-emerald-400",
        HealthStatus::Unhealthy => "text-rose-400",
        HealthStatus::Unknown => "text-slate-400",
    }
}

fn instance_status_color(status: InstanceStatus, health: HealthStatus) -> &'static str {
    match (status, health) {
        (InstanceStatus::Running, HealthStatus::Healthy) => "bg-emerald-400",
        (InstanceStatus::Running, HealthStatus::Unhealthy) => "bg-rose-400",
        (InstanceStatus::Running, HealthStatus::Unknown) => "bg-amber-400",
        (InstanceStatus::Starting, _) => "bg-sky-400",
        (InstanceStatus::Unhealthy, _) => "bg-rose-400",
        (InstanceStatus::Stopping, _) => "bg-amber-400",
        (InstanceStatus::Stopped, _) => "bg-slate-500",
    }
}

fn format_trigger(trigger: &TriggerConfig) -> (String, &'static str) {
    match trigger {
        TriggerConfig::Http { port } => (
            format!("HTTP :{}", port.unwrap_or(8080)),
            "HTTP",
        ),
        TriggerConfig::Cron { schedule } => (format!("Cron {schedule}"), "CRON"),
        TriggerConfig::Queue { topic } => (format!("Queue {topic}"), "Q"),
    }
}

// ── Builder Helpers ─────────────────────────────────────────────

pub fn build_cluster_summary(
    deployments: &[DeploymentSpec],
    all_instances: &[InstanceState],
    nodes: &[NodeInfo],
    active_rollout_count: usize,
) -> ClusterSummary {
    let mut ns_counts: HashMap<String, usize> = HashMap::new();
    for d in deployments {
        *ns_counts.entry(d.namespace.clone()).or_default() += 1;
    }
    let mut namespace_counts: Vec<(String, usize)> = ns_counts.into_iter().collect();
    namespace_counts.sort_by(|a, b| a.0.cmp(&b.0));

    let running = all_instances
        .iter()
        .filter(|i| i.status == InstanceStatus::Running)
        .count();
    let unhealthy = all_instances
        .iter()
        .filter(|i| i.status == InstanceStatus::Unhealthy || i.health == HealthStatus::Unhealthy)
        .count();
    let stopped = all_instances
        .iter()
        .filter(|i| i.status == InstanceStatus::Stopped)
        .count();

    let total_mem: u64 = nodes.iter().map(|n| n.capacity_memory_bytes).sum();
    let used_mem: u64 = nodes.iter().map(|n| n.used_memory_bytes).sum();
    let total_cpu: u32 = nodes.iter().map(|n| n.capacity_cpu_weight).sum();
    let used_cpu: u32 = nodes.iter().map(|n| n.used_cpu_weight).sum();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let nodes_ready = nodes
        .iter()
        .filter(|n| now.saturating_sub(n.last_heartbeat) <= 30)
        .count();

    ClusterSummary {
        deployment_count: deployments.len(),
        namespace_counts,
        instances: InstanceCounts {
            running,
            unhealthy,
            stopped,
            total: all_instances.len(),
        },
        node_count: nodes.len(),
        nodes_ready,
        active_rollouts: active_rollout_count,
        cluster_memory: ResourceBar::memory(used_mem, total_mem),
        cluster_cpu: ResourceBar::cpu(used_cpu, total_cpu),
    }
}

pub fn build_alerts(
    deployments: &[DeploymentView],
    rollouts: &[RolloutView],
) -> Vec<AlertView> {
    let mut alerts = Vec::new();

    for d in deployments {
        let unhealthy_count = d
            .health_dots
            .iter()
            .filter(|dot| dot.color == "bg-rose-400")
            .count();
        if unhealthy_count > 0 {
            alerts.push(AlertView {
                severity: "Error",
                severity_color: "border-rose-400/50 bg-rose-500/10",
                deployment: d.name.clone(),
                message: format!("{unhealthy_count} unhealthy instance(s)"),
            });
        }

        if let Some(err) = d.error_rate {
            if err > 5.0 {
                alerts.push(AlertView {
                    severity: "Warning",
                    severity_color: "border-amber-400/50 bg-amber-500/10",
                    deployment: d.name.clone(),
                    message: format!("Error rate {err:.1}%"),
                });
            }
        }
    }

    for r in rollouts {
        if r.phase_display == "Rolled Back" {
            alerts.push(AlertView {
                severity: "Error",
                severity_color: "border-rose-400/50 bg-rose-500/10",
                deployment: r.deployment_id.clone(),
                message: format!("Rollout rolled back: {}", r.progress_text),
            });
        }
    }

    alerts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1024 * 1024), "1 MB");
        assert_eq!(format_bytes(64 * 1024 * 1024), "64 MB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GB");
        assert_eq!(format_bytes(1536 * 1024 * 1024), "1.5 GB");
    }

    #[test]
    fn format_relative_time_values() {
        assert_eq!(format_relative_time(0), "never");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let result = format_relative_time(now - 30);
        assert!(result.contains("30s ago"));

        let result = format_relative_time(now - 120);
        assert!(result.contains("2m ago"));

        let result = format_relative_time(now - 7200);
        assert!(result.contains("2h ago"));

        let result = format_relative_time(now - 172800);
        assert!(result.contains("2d ago"));
    }

    #[test]
    fn format_timestamp_valid() {
        let result = format_timestamp(1000);
        assert!(result.contains("1970"));
    }

    #[test]
    fn resource_bar_percent_calculation() {
        let bar = ResourceBar::memory(512 * 1024 * 1024, 1024 * 1024 * 1024);
        assert!((bar.percent - 50.0).abs() < 0.1);
        assert_eq!(bar.bar_color(), "bg-grid-accent");

        let bar = ResourceBar::memory(800 * 1024 * 1024, 1024 * 1024 * 1024);
        assert_eq!(bar.bar_color(), "bg-grid-warn");

        let bar = ResourceBar::memory(950 * 1024 * 1024, 1024 * 1024 * 1024);
        assert_eq!(bar.bar_color(), "bg-grid-danger");
    }

    #[test]
    fn resource_bar_zero_total() {
        let bar = ResourceBar::memory(0, 0);
        assert_eq!(bar.percent, 0.0);
    }

    #[test]
    fn instance_view_from_state() {
        let state = InstanceState {
            id: "inst-0".to_string(),
            deployment_id: "default/api".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 2,
            memory_bytes: 64 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        };
        let view = InstanceView::from_state(&state);
        assert_eq!(view.status, "Running");
        assert_eq!(view.health, "Healthy");
        assert_eq!(view.status_color, "text-emerald-400");
        assert_eq!(view.memory_display, "64 MB");
    }

    #[test]
    fn deployment_view_from_spec() {
        let spec = DeploymentSpec {
            id: "default/api".to_string(),
            namespace: "default".to_string(),
            name: "api".to_string(),
            source: "file://test.wasm".to_string(),
            trigger: TriggerConfig::Http { port: Some(8080) },
            instances: warpgrid_state::InstanceConstraints { min: 1, max: 10 },
            resources: warpgrid_state::ResourceLimits {
                memory_bytes: 64 * 1024 * 1024,
                cpu_weight: 100,
            },
            scaling: None,
            health: None,
            shims: warpgrid_state::ShimsEnabled::default(),
            env: std::collections::HashMap::new(),
            created_at: 1000,
            updated_at: 1000,
        };
        let instances = vec![InstanceState {
            id: "inst-0".to_string(),
            deployment_id: "default/api".to_string(),
            node_id: "node-1".to_string(),
            status: InstanceStatus::Running,
            health: HealthStatus::Healthy,
            restart_count: 0,
            memory_bytes: 32 * 1024 * 1024,
            started_at: 1000,
            updated_at: 1000,
        }];

        let view = DeploymentView::from_spec(&spec, &instances, None);
        assert_eq!(view.name, "api");
        assert_eq!(view.instances_running, 1);
        assert_eq!(view.trigger_display, "HTTP :8080");
        assert_eq!(view.health_dots.len(), 1);
        assert_eq!(view.health_dots[0].color, "bg-emerald-400");
    }

    #[test]
    fn cluster_summary_aggregation() {
        let deployments = vec![
            DeploymentSpec {
                id: "default/a".to_string(),
                namespace: "default".to_string(),
                name: "a".to_string(),
                source: "test".to_string(),
                trigger: TriggerConfig::Http { port: None },
                instances: warpgrid_state::InstanceConstraints { min: 1, max: 5 },
                resources: warpgrid_state::ResourceLimits {
                    memory_bytes: 64 * 1024 * 1024,
                    cpu_weight: 100,
                },
                scaling: None,
                health: None,
                shims: warpgrid_state::ShimsEnabled::default(),
                env: std::collections::HashMap::new(),
                created_at: 1000,
                updated_at: 1000,
            },
        ];
        let instances = vec![
            InstanceState {
                id: "i-0".to_string(),
                deployment_id: "default/a".to_string(),
                node_id: "node-1".to_string(),
                status: InstanceStatus::Running,
                health: HealthStatus::Healthy,
                restart_count: 0,
                memory_bytes: 32 * 1024 * 1024,
                started_at: 1000,
                updated_at: 1000,
            },
        ];
        let nodes = vec![];

        let summary = build_cluster_summary(&deployments, &instances, &nodes, 0);
        assert_eq!(summary.deployment_count, 1);
        assert_eq!(summary.instances.running, 1);
        assert_eq!(summary.instances.total, 1);
    }

    #[test]
    fn build_metrics_rows_empty() {
        let rows = build_metrics_rows(&[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn build_metrics_rows_normalizes() {
        let snaps = vec![
            MetricsSnapshot {
                deployment_id: "d".to_string(),
                epoch: 1000,
                rps: 100.0,
                latency_p50_ms: 5.0,
                latency_p99_ms: 50.0,
                error_rate: 0.01,
                total_memory_bytes: 64 * 1024 * 1024,
                active_instances: 3,
            },
            MetricsSnapshot {
                deployment_id: "d".to_string(),
                epoch: 1060,
                rps: 200.0,
                latency_p50_ms: 10.0,
                latency_p99_ms: 100.0,
                error_rate: 0.03,
                total_memory_bytes: 128 * 1024 * 1024,
                active_instances: 5,
            },
        ];
        let rows = build_metrics_rows(&snaps);
        assert_eq!(rows.len(), 2);
        assert!((rows[0].rps_bar_width - 50.0).abs() < 0.1);
        assert!((rows[1].rps_bar_width - 100.0).abs() < 0.1);
    }
}
