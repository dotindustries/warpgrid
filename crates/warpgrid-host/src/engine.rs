//! WarpGridEngine — top-level orchestrator.
//!
//! Wires together all shim components (filesystem, DNS, signals, database proxy,
//! threading) and registers them with the Wasmtime linker at instantiation time.
//!
//! # Architecture
//!
//! The engine creates a `wasmtime::Engine` configured for the component model
//! and async execution. A `Linker<HostState>` is set up with host functions
//! registered conditionally based on `ShimConfig`.
//!
//! `HostState` holds the per-instance shim state. It implements all five WIT
//! Host traits by delegating to the individual shim implementations.

use std::sync::Arc;

use wasmtime::component::{HasSelf, Linker};
use wasmtime::{Config, Engine};

use crate::bindings::warpgrid::shim;
use crate::bindings::WarpgridShims;
use crate::config::ShimConfig;
use crate::db_proxy::host::DbProxyHost;
use crate::db_proxy::{ConnectionFactory, ConnectionPoolManager};
use crate::dns::host::DnsHost;
use crate::dns::DnsResolver;
use crate::filesystem::host::FilesystemHost;
use crate::filesystem::VirtualFileMap;

/// Per-instance host state.
///
/// Each `Store<HostState>` gets its own `HostState`, which holds the shim
/// implementations for that instance. Optional fields correspond to shims
/// that may be disabled in the config.
pub struct HostState {
    pub filesystem: Option<FilesystemHost>,
    pub dns: Option<DnsHost>,
    pub db_proxy: Option<DbProxyHost>,
    /// Pending signal queue (FIFO).
    pub signal_queue: Vec<shim::signals::SignalType>,
    /// Declared threading model (set by guest).
    pub threading_model: Option<shim::threading::ThreadingModel>,
    /// Optional resource limiter for memory/table enforcement.
    /// Uses `wasmtime::StoreLimits` for compatibility with `Store::limiter()`.
    pub limiter: Option<wasmtime::StoreLimits>,
}

// ── Host trait implementations ─────────────────────────────────────

impl shim::filesystem::Host for HostState {
    fn open_virtual(&mut self, path: String) -> Result<u64, String> {
        self.filesystem
            .as_mut()
            .ok_or_else(|| "filesystem shim not enabled".to_string())
            .and_then(|fs| fs.open_virtual(path))
    }

    fn read_virtual(&mut self, handle: u64, len: u32) -> Result<Vec<u8>, String> {
        self.filesystem
            .as_mut()
            .ok_or_else(|| "filesystem shim not enabled".to_string())
            .and_then(|fs| fs.read_virtual(handle, len))
    }

    fn stat_virtual(&mut self, path: String) -> Result<shim::filesystem::FileStat, String> {
        self.filesystem
            .as_mut()
            .ok_or_else(|| "filesystem shim not enabled".to_string())
            .and_then(|fs| fs.stat_virtual(path))
    }

    fn close_virtual(&mut self, handle: u64) -> Result<(), String> {
        self.filesystem
            .as_mut()
            .ok_or_else(|| "filesystem shim not enabled".to_string())
            .and_then(|fs| fs.close_virtual(handle))
    }
}

impl shim::dns::Host for HostState {
    fn resolve_address(
        &mut self,
        hostname: String,
    ) -> Result<Vec<shim::dns::IpAddressRecord>, String> {
        self.dns
            .as_mut()
            .ok_or_else(|| "dns shim not enabled".to_string())
            .and_then(|dns| dns.resolve_address(hostname))
    }
}

impl shim::signals::Host for HostState {
    fn on_signal(&mut self, signal: shim::signals::SignalType) -> Result<(), String> {
        self.signal_queue.push(signal);
        Ok(())
    }

    fn poll_signal(&mut self) -> Option<shim::signals::SignalType> {
        if self.signal_queue.is_empty() {
            None
        } else {
            Some(self.signal_queue.remove(0))
        }
    }
}

impl shim::database_proxy::Host for HostState {
    fn connect(
        &mut self,
        config: shim::database_proxy::ConnectConfig,
    ) -> Result<u64, String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "database proxy shim not enabled".to_string())
            .and_then(|db| db.connect(config))
    }

    fn send(&mut self, handle: u64, data: Vec<u8>) -> Result<u32, String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "database proxy shim not enabled".to_string())
            .and_then(|db| db.send(handle, data))
    }

    fn recv(&mut self, handle: u64, max_bytes: u32) -> Result<Vec<u8>, String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "database proxy shim not enabled".to_string())
            .and_then(|db| db.recv(handle, max_bytes))
    }

    fn close(&mut self, handle: u64) -> Result<(), String> {
        self.db_proxy
            .as_mut()
            .ok_or_else(|| "database proxy shim not enabled".to_string())
            .and_then(|db| db.close(handle))
    }
}

impl shim::threading::Host for HostState {
    fn declare_threading_model(
        &mut self,
        model: shim::threading::ThreadingModel,
    ) -> Result<(), String> {
        tracing::info!(?model, "guest declared threading model");
        self.threading_model = Some(model);
        Ok(())
    }
}

// ── WarpGridEngine ─────────────────────────────────────────────────

/// The top-level engine that configures Wasmtime and sets up the linker.
///
/// `WarpGridEngine` is cheap to clone (holds `Arc` references internally).
#[derive(Clone)]
pub struct WarpGridEngine {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
}

impl WarpGridEngine {
    /// Create a new `WarpGridEngine` with the default async + component-model config.
    pub fn new() -> anyhow::Result<Self> {
        let mut config = Config::new();
        config.async_support(true);
        config.wasm_component_model(true);
        config.wasm_component_model_async(true);

        let engine = Engine::new(&config)?;
        let mut linker = Linker::new(&engine);

        // Register all WIT host functions at the world level.
        // Explicit type parameters resolve ambiguity between HostState
        // and individual shim hosts (FilesystemHost, DnsHost, etc.)
        // that also implement the Host traits for unit testing.
        WarpgridShims::add_to_linker::<HostState, HasSelf<HostState>>(
            &mut linker,
            |state: &mut HostState| state,
        )?;

        tracing::info!("WarpGrid engine initialized");

        Ok(Self {
            engine,
            linker: Arc::new(linker),
        })
    }

    /// Get a reference to the underlying `wasmtime::Engine`.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Get a reference to the configured `Linker`.
    pub fn linker(&self) -> &Linker<HostState> {
        &self.linker
    }

    /// Build a `HostState` from a `ShimConfig`.
    ///
    /// This creates the per-instance shim implementations based on which
    /// shims are enabled in the config.
    pub fn build_host_state(
        &self,
        config: &ShimConfig,
        connection_factory: Option<Arc<dyn ConnectionFactory>>,
    ) -> HostState {
        let filesystem = if config.filesystem {
            // Use the default virtual file map which includes /dev/null,
            // /dev/urandom, /etc/resolv.conf, /proc/self/, and timezone data.
            let file_map = Arc::new(VirtualFileMap::with_defaults());
            Some(FilesystemHost::new(file_map))
        } else {
            None
        };

        let dns = if config.dns {
            let resolver = Arc::new(DnsResolver::new(
                config.service_registry.clone(),
                &config.etc_hosts_content,
            ));
            let runtime_handle = tokio::runtime::Handle::current();
            Some(DnsHost::new(resolver, runtime_handle))
        } else {
            None
        };

        let db_proxy = if config.database_proxy {
            if let Some(factory) = connection_factory {
                let pool_manager =
                    Arc::new(ConnectionPoolManager::new(config.pool_config.clone(), factory));
                let runtime_handle = tokio::runtime::Handle::current();
                Some(DbProxyHost::new(pool_manager, runtime_handle))
            } else {
                tracing::warn!("database_proxy enabled but no connection factory provided");
                None
            }
        } else {
            None
        };

        HostState {
            filesystem,
            dns,
            db_proxy,
            signal_queue: Vec::new(),
            threading_model: None,
            limiter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_creates_successfully() {
        let engine = WarpGridEngine::new();
        assert!(engine.is_ok());
    }

    #[test]
    fn host_state_with_no_shims() {
        let config = ShimConfig {
            filesystem: false,
            dns: false,
            signals: false,
            database_proxy: false,
            threading: false,
            ..ShimConfig::default()
        };

        // We need a tokio runtime for DNS even though it's disabled,
        // because build_host_state checks config.dns.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let engine = WarpGridEngine::new().unwrap();
        let state = engine.build_host_state(&config, None);

        assert!(state.filesystem.is_none());
        assert!(state.dns.is_none());
        assert!(state.db_proxy.is_none());
    }

    #[test]
    fn host_state_with_filesystem_and_dns() {
        let config = ShimConfig::default();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let engine = WarpGridEngine::new().unwrap();
        let state = engine.build_host_state(&config, None);

        assert!(state.filesystem.is_some());
        assert!(state.dns.is_some());
        assert!(state.db_proxy.is_none()); // no connection factory provided
    }

    #[test]
    fn disabled_shim_returns_error() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signal_queue: Vec::new(),
            threading_model: None,
            limiter: None,
        };

        let result = shim::filesystem::Host::open_virtual(&mut state, "/etc/hosts".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not enabled"));

        let result = shim::dns::Host::resolve_address(&mut state, "example.com".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn signal_queue_fifo() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signal_queue: Vec::new(),
            threading_model: None,
            limiter: None,
        };

        shim::signals::Host::on_signal(&mut state, shim::signals::SignalType::Terminate).unwrap();
        shim::signals::Host::on_signal(&mut state, shim::signals::SignalType::Hangup).unwrap();

        assert_eq!(
            shim::signals::Host::poll_signal(&mut state),
            Some(shim::signals::SignalType::Terminate)
        );
        assert_eq!(
            shim::signals::Host::poll_signal(&mut state),
            Some(shim::signals::SignalType::Hangup)
        );
        assert_eq!(shim::signals::Host::poll_signal(&mut state), None);
    }

    #[test]
    fn threading_model_declaration() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signal_queue: Vec::new(),
            threading_model: None,
            limiter: None,
        };

        shim::threading::Host::declare_threading_model(
            &mut state,
            shim::threading::ThreadingModel::Cooperative,
        )
        .unwrap();

        assert!(matches!(
            state.threading_model,
            Some(shim::threading::ThreadingModel::Cooperative)
        ));
    }
}
