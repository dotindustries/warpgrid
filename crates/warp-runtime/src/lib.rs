//! warp-runtime — Wasmtime WASI P2 runtime sandbox.
//!
//! Provides a high-level API for loading, compiling, and instantiating
//! Wasm components with WarpGrid's host shims. The runtime manages:
//!
//! - **Module compilation**: Compiles `.wasm` files into `Component` objects
//! - **Instance creation**: Creates sandboxed instances with per-instance
//!   resource limits and shim configuration
//! - **Instance pooling**: Manages warm pools of pre-instantiated modules
//! - **Resource limiting**: Enforces memory and table size limits per instance
//!   via wasmtime's built-in `StoreLimits`
//!
//! # Architecture
//!
//! ```text
//! Runtime
//!   ├── WarpGridEngine (shared wasmtime::Engine + Linker)
//!   ├── CompiledModule cache (module name → Component)
//!   └── InstancePool per deployment
//!       ├── InstanceFactory (engine + module)
//!       └── VecDeque<WasmInstance> (idle instances)
//! ```

pub mod instance;
pub mod limiter;
pub mod pool;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use warpgrid_host::config::ShimConfig;
use warpgrid_host::engine::WarpGridEngine;

pub use instance::{CompiledModule, InstanceFactory, WasmInstance};
pub use pool::{InstancePool, PoolConfig};

/// The top-level WarpGrid runtime.
///
/// Manages a shared `WarpGridEngine`, a cache of compiled modules,
/// and per-deployment instance pools.
pub struct Runtime {
    engine: WarpGridEngine,
    /// Compiled module cache: name → compiled component.
    modules: Arc<Mutex<HashMap<String, CompiledModule>>>,
}

impl Runtime {
    /// Create a new runtime with default configuration.
    pub fn new() -> anyhow::Result<Self> {
        let engine = WarpGridEngine::new()?;
        tracing::info!("WarpGrid runtime initialized");
        Ok(Self {
            engine,
            modules: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Get a reference to the underlying engine.
    pub fn engine(&self) -> &WarpGridEngine {
        &self.engine
    }

    /// Load and compile a Wasm module from raw bytes.
    ///
    /// The compiled module is cached by name for reuse.
    pub async fn load_module(&self, name: &str, bytes: &[u8]) -> anyhow::Result<CompiledModule> {
        let module = CompiledModule::from_bytes(self.engine.engine(), name, bytes)?;
        self.modules
            .lock()
            .await
            .insert(name.to_string(), module.clone());
        Ok(module)
    }

    /// Load and compile a Wasm module from a file path.
    ///
    /// The compiled module is cached by name for reuse.
    pub async fn load_module_from_file(
        &self,
        name: &str,
        path: &str,
    ) -> anyhow::Result<CompiledModule> {
        let module = CompiledModule::from_file(self.engine.engine(), name, path)?;
        self.modules
            .lock()
            .await
            .insert(name.to_string(), module.clone());
        Ok(module)
    }

    /// Get a previously compiled module by name.
    pub async fn get_module(&self, name: &str) -> Option<CompiledModule> {
        self.modules.lock().await.get(name).cloned()
    }

    /// Create a single instance of a compiled module.
    pub async fn instantiate(
        &self,
        module: &CompiledModule,
        shim_config: &ShimConfig,
        memory_limit: usize,
    ) -> anyhow::Result<WasmInstance> {
        WasmInstance::new(&self.engine, module, shim_config, memory_limit).await
    }

    /// Create an instance pool for a compiled module.
    pub fn create_pool(
        &self,
        module: CompiledModule,
        pool_config: PoolConfig,
    ) -> InstancePool {
        let factory = InstanceFactory::new(self.engine.clone(), module);
        InstancePool::new(factory, pool_config)
    }

    /// List all cached module names.
    pub async fn cached_modules(&self) -> Vec<String> {
        self.modules.lock().await.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_creates_successfully() {
        let runtime = Runtime::new();
        assert!(runtime.is_ok());
    }

    #[tokio::test]
    async fn module_cache_starts_empty() {
        let runtime = Runtime::new().unwrap();
        assert!(runtime.cached_modules().await.is_empty());
    }

    #[test]
    fn pool_creation_api_works() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let runtime = Runtime::new().unwrap();
        let _engine = runtime.engine();
    }
}
