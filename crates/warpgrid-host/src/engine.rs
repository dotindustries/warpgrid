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

use wasmtime::component::{Component, HasSelf, Instance, Linker};
use wasmtime::{Config, Engine, Store, StoreLimitsBuilder};

use crate::bindings::async_handler_bindings::warpgrid::shim::http_types;
use crate::bindings::warpgrid::shim;
use crate::config::ShimConfig;
use crate::db_proxy::host::DbProxyHost;
use crate::db_proxy::{AsyncConnectionFactory, ConnectionFactory, ConnectionPoolManager};
use crate::dns::CachedDnsResolver;
use crate::dns::host::DnsHost;
use crate::dns::DnsResolver;
use crate::filesystem::host::FilesystemHost;
use crate::filesystem::VirtualFileMap;
use crate::signals::host::SignalsHost;

/// Per-instance host state.
///
/// Each `Store<HostState>` gets its own `HostState`, which holds the shim
/// implementations for that instance. Optional fields correspond to shims
/// that may be disabled in the config.
pub struct HostState {
    pub filesystem: Option<FilesystemHost>,
    pub dns: Option<DnsHost>,
    pub db_proxy: Option<DbProxyHost>,
    /// Signal handling: interest registration, bounded queue, and filtering.
    pub signals: SignalsHost,
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
        self.signals.on_signal(signal)
    }

    fn poll_signal(&mut self) -> Option<shim::signals::SignalType> {
        self.signals.poll_signal()
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
        if self.threading_model.is_some() {
            return Err("threading model already declared".to_string());
        }

        match model {
            shim::threading::ThreadingModel::ParallelRequired => {
                tracing::warn!(
                    ?model,
                    "parallel threading requested but not supported; execution will use cooperative mode"
                );
            }
            shim::threading::ThreadingModel::Cooperative => {
                tracing::info!(?model, "cooperative threading model declared");
            }
        }

        self.threading_model = Some(model);
        Ok(())
    }
}

/// The `http-types` interface defines only types (no functions), but
/// the bindgen! macro still generates a Host trait for interface-level
/// dispatch. This empty implementation satisfies the trait bound.
impl http_types::Host for HostState {}

// ── WarpGridEngine ─────────────────────────────────────────────────

/// The top-level engine that configures Wasmtime and sets up the linker.
///
/// `WarpGridEngine` is cheap to clone (holds `Arc` references internally).
/// Stores a `ShimConfig` that controls which shim interfaces are registered
/// with the linker and how `HostState` is constructed.
#[derive(Clone)]
pub struct WarpGridEngine {
    engine: Engine,
    linker: Arc<Linker<HostState>>,
    config: ShimConfig,
}

impl WarpGridEngine {
    /// Create a new `WarpGridEngine` with the given shim configuration.
    ///
    /// Only shim interfaces enabled in `config` are registered with the linker.
    /// If a guest component imports a disabled interface, instantiation will
    /// fail at link time (expected behavior).
    pub fn new(config: ShimConfig) -> anyhow::Result<Self> {
        let mut wasm_config = Config::new();
        wasm_config.async_support(true);
        wasm_config.wasm_component_model(true);
        wasm_config.wasm_component_model_async(true);

        let engine = Engine::new(&wasm_config)?;
        let mut linker = Linker::new(&engine);

        // Register only enabled shim interfaces with the linker.
        // Each per-interface `add_to_linker` is generated by the bindgen! macro.
        Self::register_shim_interfaces(&config, &mut linker)?;

        tracing::info!(
            filesystem = config.filesystem,
            dns = config.dns,
            signals = config.signals,
            database_proxy = config.database_proxy,
            threading = config.threading,
            dns_cache_ttl_seconds = config.dns_config.ttl_seconds,
            dns_cache_max_entries = config.dns_config.cache_size,
            db_pool_size = config.database_proxy_config.pool_size,
            fs_timezone = %config.filesystem_config.timezone_name,
            "WarpGrid engine initialized"
        );

        Ok(Self {
            engine,
            linker: Arc::new(linker),
            config,
        })
    }

    /// Register shim interfaces with a linker based on config flags.
    fn register_shim_interfaces(
        config: &ShimConfig,
        linker: &mut Linker<HostState>,
    ) -> anyhow::Result<()> {
        if config.filesystem {
            shim::filesystem::add_to_linker::<HostState, HasSelf<HostState>>(
                linker,
                |state: &mut HostState| state,
            )?;
        }
        if config.dns {
            shim::dns::add_to_linker::<HostState, HasSelf<HostState>>(
                linker,
                |state: &mut HostState| state,
            )?;
        }
        if config.signals {
            shim::signals::add_to_linker::<HostState, HasSelf<HostState>>(
                linker,
                |state: &mut HostState| state,
            )?;
        }
        if config.database_proxy {
            shim::database_proxy::add_to_linker::<HostState, HasSelf<HostState>>(
                linker,
                |state: &mut HostState| state,
            )?;
        }
        if config.threading {
            shim::threading::add_to_linker::<HostState, HasSelf<HostState>>(
                linker,
                |state: &mut HostState| state,
            )?;
        }
        Ok(())
    }

    /// Get a reference to the stored `ShimConfig`.
    pub fn config(&self) -> &ShimConfig {
        &self.config
    }

    /// Get a reference to the underlying `wasmtime::Engine`.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Get a reference to the configured `Linker` (shim-only world).
    pub fn linker(&self) -> &Linker<HostState> {
        &self.linker
    }

    /// Create a new `Linker` configured for the async handler world.
    ///
    /// The async handler world extends the base shim world with an exported
    /// `handle-request` function. Components instantiated with this linker
    /// can be invoked as HTTP request handlers.
    ///
    /// Only shim interfaces enabled in the stored config are registered.
    /// The `http-types` interface (types only, no functions) is always registered.
    pub fn async_handler_linker(&self) -> anyhow::Result<Linker<HostState>> {
        let mut linker = Linker::new(&self.engine);

        // Register only enabled shim interfaces (same as main linker).
        Self::register_shim_interfaces(&self.config, &mut linker)?;

        // The http-types interface defines only types (no functions).
        // Always register it for the async handler world.
        http_types::add_to_linker::<HostState, HasSelf<HostState>>(
            &mut linker,
            |state: &mut HostState| state,
        )?;

        tracing::info!("async handler linker initialized");
        Ok(linker)
    }

    /// Compile and instantiate a Wasm component from raw bytes in one call.
    ///
    /// This is a convenience method that:
    /// 1. Compiles the component from bytes
    /// 2. Builds host state from the stored config
    /// 3. Creates a store with a 64 MiB memory limit
    /// 4. Instantiates the component via the linker
    ///
    /// For more control over memory limits or connection factories, use the
    /// individual methods (`build_host_state`, `linker().instantiate_async`).
    pub async fn instantiate(
        &self,
        module_bytes: &[u8],
    ) -> anyhow::Result<(Store<HostState>, Instance)> {
        let component = Component::from_binary(&self.engine, module_bytes)?;

        let mut host_state = self.build_host_state(None);

        // Default memory limit: 64 MiB.
        let limits = StoreLimitsBuilder::new()
            .memory_size(64 * 1024 * 1024)
            .table_elements(10_000)
            .build();
        host_state.limiter = Some(limits);

        let mut store = Store::new(&self.engine, host_state);
        store.limiter(|data| {
            data.limiter
                .as_mut()
                .expect("limiter must be set before instantiation")
        });

        let instance = self.linker.instantiate_async(&mut store, &component).await?;

        Ok((store, instance))
    }

    /// Build a `HostState` from the stored `ShimConfig`.
    ///
    /// This creates the per-instance shim implementations based on which
    /// shims are enabled in the config.
    ///
    /// When an `async_factory` is provided, the database proxy pool manager
    /// uses async I/O for connections, releasing the internal mutex during
    /// send/recv to enable concurrent access across connections.
    pub fn build_host_state(
        &self,
        connection_factory: Option<Arc<dyn ConnectionFactory>>,
    ) -> HostState {
        self.build_host_state_with_async(connection_factory, None)
    }

    /// Build a `HostState` with both sync and async connection factories.
    ///
    /// When `async_factory` is `Some`, the pool manager prefers async I/O
    /// for new connections and uses `send_query`/`receive_results` which
    /// release the mutex during I/O operations.
    pub fn build_host_state_with_async(
        &self,
        connection_factory: Option<Arc<dyn ConnectionFactory>>,
        async_factory: Option<Arc<dyn AsyncConnectionFactory>>,
    ) -> HostState {
        let config = &self.config;

        let filesystem = if config.filesystem {
            // Use the default virtual file map which includes /dev/null,
            // /dev/urandom, /etc/resolv.conf, /proc/self/, and timezone data.
            let file_map = Arc::new(VirtualFileMap::with_defaults());
            Some(FilesystemHost::new(file_map))
        } else {
            None
        };

        let dns = if config.dns {
            let resolver = DnsResolver::new(
                config.service_registry.clone(),
                &config.etc_hosts_content,
            );
            let cached = Arc::new(CachedDnsResolver::new(
                resolver,
                config.dns_cache_config.clone(),
            ));
            let runtime_handle = tokio::runtime::Handle::current();
            Some(DnsHost::new(cached, runtime_handle))
        } else {
            None
        };

        let db_proxy = if config.database_proxy {
            if let Some(factory) = connection_factory {
                let pool_manager = if let Some(async_f) = async_factory {
                    Arc::new(ConnectionPoolManager::new_with_async(
                        config.pool_config.clone(),
                        factory,
                        async_f,
                    ))
                } else {
                    Arc::new(ConnectionPoolManager::new(config.pool_config.clone(), factory))
                };
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
            signals: SignalsHost::new(),
            threading_model: None,
            limiter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_creates_with_default_config() {
        let engine = WarpGridEngine::new(ShimConfig::default());
        assert!(engine.is_ok());
    }

    #[test]
    fn engine_creates_with_all_shims_disabled() {
        let config = ShimConfig {
            filesystem: false,
            dns: false,
            signals: false,
            database_proxy: false,
            threading: false,
            ..ShimConfig::default()
        };
        let engine = WarpGridEngine::new(config);
        assert!(engine.is_ok());
    }

    #[test]
    fn engine_stores_and_exposes_config() {
        let config = ShimConfig {
            filesystem: true,
            dns: false,
            signals: true,
            database_proxy: false,
            threading: true,
            ..ShimConfig::default()
        };
        let engine = WarpGridEngine::new(config).unwrap();

        assert!(engine.config().filesystem);
        assert!(!engine.config().dns);
        assert!(engine.config().signals);
        assert!(!engine.config().database_proxy);
        assert!(engine.config().threading);
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

        let engine = WarpGridEngine::new(config).unwrap();
        let state = engine.build_host_state(None);

        assert!(state.filesystem.is_none());
        assert!(state.dns.is_none());
        assert!(state.db_proxy.is_none());
    }

    #[test]
    fn host_state_with_filesystem_and_dns() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let engine = WarpGridEngine::new(ShimConfig::default()).unwrap();
        let state = engine.build_host_state(None);

        assert!(state.filesystem.is_some());
        assert!(state.dns.is_some());
        assert!(state.db_proxy.is_none()); // no connection factory provided
    }

    #[test]
    fn host_state_uses_stored_config() {
        let config = ShimConfig {
            filesystem: true,
            dns: false,
            signals: true,
            database_proxy: false,
            threading: false,
            ..ShimConfig::default()
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let engine = WarpGridEngine::new(config).unwrap();
        let state = engine.build_host_state(None);

        assert!(state.filesystem.is_some());
        assert!(state.dns.is_none());
        assert!(state.db_proxy.is_none());
    }

    #[test]
    fn disabled_shim_returns_error() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signals: SignalsHost::new(),
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
    fn signal_fifo() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signals: SignalsHost::new(),
            threading_model: None,
            limiter: None,
        };

        // Register interest in both signal types via the Host trait
        shim::signals::Host::on_signal(&mut state, shim::signals::SignalType::Terminate).unwrap();
        shim::signals::Host::on_signal(&mut state, shim::signals::SignalType::Hangup).unwrap();

        // Deliver signals via the host-side API
        state.signals.deliver_signal(shim::signals::SignalType::Terminate);
        state.signals.deliver_signal(shim::signals::SignalType::Hangup);

        // Poll via the Host trait — should return FIFO order
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
    fn async_handler_linker_creates_successfully() {
        let engine = WarpGridEngine::new(ShimConfig::default()).unwrap();
        let linker = engine.async_handler_linker();
        assert!(linker.is_ok());
    }

    #[test]
    fn async_handler_linker_with_selective_shims() {
        let config = ShimConfig {
            filesystem: true,
            dns: false,
            signals: false,
            database_proxy: false,
            threading: false,
            ..ShimConfig::default()
        };
        let engine = WarpGridEngine::new(config).unwrap();
        let linker = engine.async_handler_linker();
        assert!(linker.is_ok());
    }

    #[test]
    fn threading_model_declaration() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signals: SignalsHost::new(),
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

    #[test]
    fn threading_model_parallel_required_succeeds() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signals: SignalsHost::new(),
            threading_model: None,
            limiter: None,
        };

        shim::threading::Host::declare_threading_model(
            &mut state,
            shim::threading::ThreadingModel::ParallelRequired,
        )
        .unwrap();

        assert!(matches!(
            state.threading_model,
            Some(shim::threading::ThreadingModel::ParallelRequired)
        ));
    }

    #[test]
    fn threading_model_double_declaration_errors() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signals: SignalsHost::new(),
            threading_model: None,
            limiter: None,
        };

        shim::threading::Host::declare_threading_model(
            &mut state,
            shim::threading::ThreadingModel::Cooperative,
        )
        .unwrap();

        let result = shim::threading::Host::declare_threading_model(
            &mut state,
            shim::threading::ThreadingModel::ParallelRequired,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already declared"));

        // Original model is preserved
        assert!(matches!(
            state.threading_model,
            Some(shim::threading::ThreadingModel::Cooperative)
        ));
    }

    #[tokio::test]
    async fn instantiate_rejects_invalid_bytes() {
        let engine = WarpGridEngine::new(ShimConfig::default()).unwrap();
        let result = engine.instantiate(b"not a valid wasm component").await;
        assert!(result.is_err());
    }

    #[test]
    fn disabled_db_proxy_host_methods_return_error() {
        let mut state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signals: SignalsHost::new(),
            threading_model: None,
            limiter: None,
        };

        let connect_config = shim::database_proxy::ConnectConfig {
            host: "db.local".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            user: "app".to_string(),
            password: None,
        };

        let connect_err = shim::database_proxy::Host::connect(&mut state, connect_config);
        assert!(connect_err.is_err());
        assert!(connect_err.unwrap_err().contains("not enabled"));

        let send_err = shim::database_proxy::Host::send(&mut state, 1, vec![0x00]);
        assert!(send_err.is_err());
        assert!(send_err.unwrap_err().contains("not enabled"));

        let recv_err = shim::database_proxy::Host::recv(&mut state, 1, 1024);
        assert!(recv_err.is_err());
        assert!(recv_err.unwrap_err().contains("not enabled"));

        let close_err = shim::database_proxy::Host::close(&mut state, 1);
        assert!(close_err.is_err());
        assert!(close_err.unwrap_err().contains("not enabled"));
    }

    #[test]
    fn build_host_state_with_db_proxy_enabled_and_factory() {
        use crate::db_proxy::{ConnectionBackend, ConnectionFactory, PoolKey};

        #[derive(Debug)]
        struct StubBackend;

        impl ConnectionBackend for StubBackend {
            fn send(&mut self, data: &[u8]) -> Result<usize, String> {
                Ok(data.len())
            }
            fn recv(&mut self, _max: usize) -> Result<Vec<u8>, String> {
                Ok(vec![])
            }
            fn ping(&mut self) -> bool {
                true
            }
            fn close(&mut self) {}
        }

        struct StubFactory;

        impl ConnectionFactory for StubFactory {
            fn connect(
                &self,
                _key: &PoolKey,
                _password: Option<&str>,
            ) -> Result<Box<dyn ConnectionBackend>, String> {
                Ok(Box::new(StubBackend))
            }
        }

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let config = ShimConfig {
            database_proxy: true,
            dns: false,
            ..ShimConfig::default()
        };
        let engine = WarpGridEngine::new(config).unwrap();
        let state = engine.build_host_state(Some(Arc::new(StubFactory)));

        assert!(
            state.db_proxy.is_some(),
            "db_proxy should be Some when enabled with a factory"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_host_state_with_async_factory_uses_async_path() {
        use crate::db_proxy::async_io::{AsyncConnectionBackend, AsyncConnectionFactory, AsyncConnectFuture};
        use crate::db_proxy::{ConnectionBackend, ConnectionFactory, PoolKey};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicU64, Ordering};

        #[derive(Debug)]
        struct StubBackend;

        impl ConnectionBackend for StubBackend {
            fn send(&mut self, data: &[u8]) -> Result<usize, String> { Ok(data.len()) }
            fn recv(&mut self, _max: usize) -> Result<Vec<u8>, String> { Ok(vec![]) }
            fn ping(&mut self) -> bool { true }
            fn close(&mut self) {}
        }

        struct CountingFactory(AtomicU64);
        impl ConnectionFactory for CountingFactory {
            fn connect(&self, _key: &PoolKey, _password: Option<&str>) -> Result<Box<dyn ConnectionBackend>, String> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Ok(Box::new(StubBackend))
            }
        }

        #[derive(Debug)]
        struct StubAsyncBackend;
        impl AsyncConnectionBackend for StubAsyncBackend {
            fn send_async<'a>(&'a mut self, data: &'a [u8]) -> Pin<Box<dyn Future<Output = Result<usize, String>> + Send + 'a>> {
                Box::pin(async move { Ok(data.len()) })
            }
            fn recv_async<'a>(&'a mut self, max_bytes: usize) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, String>> + Send + 'a>> {
                Box::pin(async move { Ok(vec![0x42; max_bytes.min(4)]) })
            }
            fn ping_async(&mut self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>> {
                Box::pin(async { true })
            }
            fn close_async(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(async {})
            }
        }

        struct CountingAsyncFactory(AtomicU64);
        impl AsyncConnectionFactory for CountingAsyncFactory {
            fn connect_async<'a>(&'a self, _key: &'a PoolKey, _password: Option<&'a str>) -> AsyncConnectFuture<'a> {
                self.0.fetch_add(1, Ordering::Relaxed);
                Box::pin(async { Ok(Box::new(StubAsyncBackend) as Box<dyn AsyncConnectionBackend>) })
            }
        }

        let sync_factory = Arc::new(CountingFactory(AtomicU64::new(0)));
        let async_factory = Arc::new(CountingAsyncFactory(AtomicU64::new(0)));

        let config = ShimConfig {
            database_proxy: true,
            dns: false,
            ..ShimConfig::default()
        };
        let engine = WarpGridEngine::new(config).unwrap();
        let mut state = engine.build_host_state_with_async(
            Some(sync_factory.clone()),
            Some(async_factory.clone()),
        );

        assert!(state.db_proxy.is_some(), "db_proxy should be Some");

        // Connect through HostState should use the async factory, not the sync one.
        let connect_config = shim::database_proxy::ConnectConfig {
            host: "db.local".into(),
            port: 5432,
            database: "mydb".into(),
            user: "app".into(),
            password: None,
        };
        let handle = shim::database_proxy::Host::connect(&mut state, connect_config).unwrap();
        assert_eq!(async_factory.0.load(Ordering::Relaxed), 1, "async factory should be called");
        assert_eq!(sync_factory.0.load(Ordering::Relaxed), 0, "sync factory should NOT be called");

        // Send/recv should work through async path.
        let sent = shim::database_proxy::Host::send(&mut state, handle, b"SELECT 1".to_vec()).unwrap();
        assert_eq!(sent, 8);

        let data = shim::database_proxy::Host::recv(&mut state, handle, 1024).unwrap();
        assert!(!data.is_empty());
    }
}
