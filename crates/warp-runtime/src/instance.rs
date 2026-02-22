//! WasmInstance — a running Wasm component instance.
//!
//! Wraps a `wasmtime::component::Instance` with its associated `Store`
//! and provides a typed interface for interacting with the guest.

use wasmtime::component::{Component, Instance};
use wasmtime::{Engine, StoreLimitsBuilder, Store};

use warpgrid_host::config::ShimConfig;
use warpgrid_host::engine::{HostState, WarpGridEngine};

/// A loaded and compiled Wasm component, ready to be instantiated.
///
/// Components are expensive to compile but cheap to instantiate.
/// Cache and reuse `CompiledModule` across instances of the same deployment.
#[derive(Clone)]
pub struct CompiledModule {
    /// The compiled component.
    component: Component,
    /// Human-readable name for logging.
    name: String,
}

impl CompiledModule {
    /// Compile a Wasm component from raw bytes.
    pub fn from_bytes(engine: &Engine, name: &str, bytes: &[u8]) -> anyhow::Result<Self> {
        let component = Component::from_binary(engine, bytes)?;
        tracing::info!(%name, "compiled wasm component");
        Ok(Self {
            component,
            name: name.to_string(),
        })
    }

    /// Compile a Wasm component from a file path.
    pub fn from_file(engine: &Engine, name: &str, path: &str) -> anyhow::Result<Self> {
        let component = Component::from_file(engine, path)?;
        tracing::info!(%name, %path, "compiled wasm component from file");
        Ok(Self {
            component,
            name: name.to_string(),
        })
    }

    /// The module name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Access the underlying component.
    pub fn component(&self) -> &Component {
        &self.component
    }
}

/// A running Wasm component instance with its store.
pub struct WasmInstance {
    store: Store<HostState>,
    _instance: Instance,
    module_name: String,
}

impl WasmInstance {
    /// Instantiate a compiled module with the given shim config.
    ///
    /// This creates a new `Store` with a fresh `HostState` and `StoreLimits`,
    /// then instantiates the component using the engine's pre-configured linker.
    pub async fn new(
        warpgrid_engine: &WarpGridEngine,
        module: &CompiledModule,
        shim_config: &ShimConfig,
        memory_limit: usize,
    ) -> anyhow::Result<Self> {
        let mut host_state = warpgrid_engine.build_host_state(shim_config, None);

        // Configure memory and table limits via wasmtime's built-in StoreLimits.
        let limits = StoreLimitsBuilder::new()
            .memory_size(memory_limit)
            .table_elements(10_000)
            .build();
        host_state.limiter = Some(limits);

        let mut store = Store::new(warpgrid_engine.engine(), host_state);
        store.limiter(|data| {
            data.limiter
                .as_mut()
                .expect("limiter must be set before instantiation")
        });

        let instance = warpgrid_engine
            .linker()
            .instantiate_async(&mut store, &module.component)
            .await?;

        tracing::info!(name = %module.name, "wasm instance created");

        Ok(Self {
            store,
            _instance: instance,
            module_name: module.name.clone(),
        })
    }

    /// Get a reference to the store (for calling exported functions).
    pub fn store(&self) -> &Store<HostState> {
        &self.store
    }

    /// Get a mutable reference to the store.
    pub fn store_mut(&mut self) -> &mut Store<HostState> {
        &mut self.store
    }

    /// The module name this instance was created from.
    pub fn module_name(&self) -> &str {
        &self.module_name
    }
}

/// Shared handle to a pre-configured engine + compiled module.
///
/// Used by the instance pool to create new instances on demand.
#[derive(Clone)]
pub struct InstanceFactory {
    engine: WarpGridEngine,
    module: CompiledModule,
}

impl InstanceFactory {
    /// Create a new factory for instantiating a specific module.
    pub fn new(engine: WarpGridEngine, module: CompiledModule) -> Self {
        Self { engine, module }
    }

    /// Create a new instance with the given config.
    pub async fn create_instance(
        &self,
        shim_config: &ShimConfig,
        memory_limit: usize,
    ) -> anyhow::Result<WasmInstance> {
        WasmInstance::new(&self.engine, &self.module, shim_config, memory_limit).await
    }

    /// The compiled module this factory produces instances of.
    pub fn module(&self) -> &CompiledModule {
        &self.module
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_creates_successfully() {
        // WarpGridEngine::new() configures async + component-model.
        // If it returns Ok, the engine is properly configured.
        let engine = WarpGridEngine::new();
        assert!(engine.is_ok());
    }

    #[test]
    fn host_state_with_store_limits() {
        let limits = StoreLimitsBuilder::new()
            .memory_size(64 * 1024 * 1024)
            .table_elements(10_000)
            .build();

        let state = HostState {
            filesystem: None,
            dns: None,
            db_proxy: None,
            signal_queue: Vec::new(),
            threading_model: None,
            limiter: Some(limits),
        };
        assert!(state.limiter.is_some());
    }
}
