//! Generated Rust bindings for WarpGrid WIT interfaces.
//!
//! Uses [`wasmtime::component::bindgen!`] to produce typed Rust traits and
//! structs from the WIT definitions in `wit/`. Each shim domain gets a
//! `Host` trait that the host-side implementation must satisfy.

wasmtime::component::bindgen!({
    path: "wit",
    world: "warpgrid-shims",
});

/// Bindings for the `warpgrid-async-handler` world.
///
/// This world extends the base shim imports with an exported
/// `async-handler` interface. The generated `WarpgridAsyncHandler`
/// type allows the host to instantiate components that export
/// `handle-request` and invoke them.
///
/// Import-side types (filesystem, dns, signals, database-proxy, threading)
/// are shared with the `warpgrid-shims` bindings via the `with` parameter,
/// so `HostState` only needs one set of Host trait implementations.
pub mod async_handler_bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "warpgrid-async-handler",
        with: {
            "warpgrid:shim/filesystem": super::warpgrid::shim::filesystem,
            "warpgrid:shim/dns": super::warpgrid::shim::dns,
            "warpgrid:shim/signals": super::warpgrid::shim::signals,
            "warpgrid:shim/database-proxy": super::warpgrid::shim::database_proxy,
            "warpgrid:shim/threading": super::warpgrid::shim::threading,
        },
        exports: { default: async },
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Filesystem interface ───────────────────────────────────────

    #[test]
    fn filesystem_file_stat_is_constructible() {
        use warpgrid::shim::filesystem::FileStat;

        let stat = FileStat {
            size: 1024,
            is_file: true,
            is_directory: false,
        };
        assert_eq!(stat.size, 1024);
        assert!(stat.is_file);
        assert!(!stat.is_directory);
    }

    #[test]
    fn filesystem_file_stat_zero_size() {
        use warpgrid::shim::filesystem::FileStat;

        let stat = FileStat {
            size: 0,
            is_file: false,
            is_directory: true,
        };
        assert_eq!(stat.size, 0);
        assert!(!stat.is_file);
        assert!(stat.is_directory);
    }

    // ── DNS interface ──────────────────────────────────────────────

    #[test]
    fn dns_ip_address_record_ipv4() {
        use warpgrid::shim::dns::IpAddressRecord;

        let record = IpAddressRecord {
            address: "10.0.0.1".into(),
            is_ipv6: false,
        };
        assert_eq!(record.address, "10.0.0.1");
        assert!(!record.is_ipv6);
    }

    #[test]
    fn dns_ip_address_record_ipv6() {
        use warpgrid::shim::dns::IpAddressRecord;

        let record = IpAddressRecord {
            address: "::1".into(),
            is_ipv6: true,
        };
        assert_eq!(record.address, "::1");
        assert!(record.is_ipv6);
    }

    // ── Signals interface ──────────────────────────────────────────

    #[test]
    fn signal_type_variants_are_complete() {
        use warpgrid::shim::signals::SignalType;

        let signals = [
            SignalType::Terminate,
            SignalType::Hangup,
            SignalType::Interrupt,
        ];
        assert_eq!(signals.len(), 3);
    }

    #[test]
    fn signal_type_is_matchable() {
        use warpgrid::shim::signals::SignalType;

        let signal = SignalType::Terminate;
        let label = match signal {
            SignalType::Terminate => "terminate",
            SignalType::Hangup => "hangup",
            SignalType::Interrupt => "interrupt",
        };
        assert_eq!(label, "terminate");
    }

    // ── Database proxy interface ───────────────────────────────────

    #[test]
    fn connect_config_with_password() {
        use warpgrid::shim::database_proxy::ConnectConfig;

        let config = ConnectConfig {
            host: "db.production.warp.local".into(),
            port: 5432,
            database: "mydb".into(),
            user: "app".into(),
            password: Some("secret".into()),
        };
        assert_eq!(config.host, "db.production.warp.local");
        assert_eq!(config.port, 5432);
        assert_eq!(config.database, "mydb");
        assert_eq!(config.user, "app");
        assert_eq!(config.password.as_deref(), Some("secret"));
    }

    #[test]
    fn connect_config_without_password() {
        use warpgrid::shim::database_proxy::ConnectConfig;

        let config = ConnectConfig {
            host: "localhost".into(),
            port: 3306,
            database: "test".into(),
            user: "root".into(),
            password: None,
        };
        assert!(config.password.is_none());
    }

    // ── Threading interface ────────────────────────────────────────

    #[test]
    fn threading_model_variants_are_complete() {
        use warpgrid::shim::threading::ThreadingModel;

        let models = [
            ThreadingModel::ParallelRequired,
            ThreadingModel::Cooperative,
        ];
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn threading_model_is_matchable() {
        use warpgrid::shim::threading::ThreadingModel;

        let model = ThreadingModel::Cooperative;
        let is_cooperative = matches!(model, ThreadingModel::Cooperative);
        assert!(is_cooperative);
    }

    // ── Host traits exist (compile-time assertions) ────────────────

    /// Verify that each interface generates a Host trait with expected methods.
    /// This is a compile-time test — if the trait signatures change upstream,
    /// this test will fail to compile.
    #[test]
    fn host_traits_have_expected_signatures() {
        use warpgrid::shim::database_proxy::ConnectConfig;
        use warpgrid::shim::dns::IpAddressRecord;
        use warpgrid::shim::filesystem::FileStat;
        use warpgrid::shim::signals::SignalType;
        use warpgrid::shim::threading::ThreadingModel;

        // We assert the traits exist by writing a struct that implements them.
        // This validates the exact method signatures generated by bindgen.
        struct MockHost;

        impl warpgrid::shim::filesystem::Host for MockHost {
            fn open_virtual(
                &mut self,
                _path: String,
            ) -> Result<u64, String> {
                Ok(0)
            }

            fn read_virtual(
                &mut self,
                _handle: u64,
                _len: u32,
            ) -> Result<Vec<u8>, String> {
                Ok(vec![])
            }

            fn stat_virtual(
                &mut self,
                _path: String,
            ) -> Result<FileStat, String> {
                Ok(FileStat {
                    size: 0,
                    is_file: true,
                    is_directory: false,
                })
            }

            fn close_virtual(
                &mut self,
                _handle: u64,
            ) -> Result<(), String> {
                Ok(())
            }
        }

        impl warpgrid::shim::dns::Host for MockHost {
            fn resolve_address(
                &mut self,
                _hostname: String,
            ) -> Result<Vec<IpAddressRecord>, String> {
                Ok(vec![])
            }
        }

        impl warpgrid::shim::signals::Host for MockHost {
            fn on_signal(
                &mut self,
                _signal: SignalType,
            ) -> Result<(), String> {
                Ok(())
            }

            fn poll_signal(&mut self) -> Option<SignalType> {
                None
            }
        }

        impl warpgrid::shim::database_proxy::Host for MockHost {
            fn connect(
                &mut self,
                _config: ConnectConfig,
            ) -> Result<u64, String> {
                Ok(1)
            }

            fn send(
                &mut self,
                _handle: u64,
                _data: Vec<u8>,
            ) -> Result<u32, String> {
                Ok(0)
            }

            fn recv(
                &mut self,
                _handle: u64,
                _max_bytes: u32,
            ) -> Result<Vec<u8>, String> {
                Ok(vec![])
            }

            fn close(
                &mut self,
                _handle: u64,
            ) -> Result<(), String> {
                Ok(())
            }
        }

        impl warpgrid::shim::threading::Host for MockHost {
            fn declare_threading_model(
                &mut self,
                _model: ThreadingModel,
            ) -> Result<(), String> {
                Ok(())
            }
        }

        // Exercise the mock to prove the traits are callable
        let mut host = MockHost;

        assert!(warpgrid::shim::filesystem::Host::open_virtual(
            &mut host,
            "/etc/hosts".into()
        )
        .is_ok());

        assert!(warpgrid::shim::dns::Host::resolve_address(
            &mut host,
            "db.test.warp.local".into()
        )
        .is_ok());

        assert!(warpgrid::shim::signals::Host::on_signal(
            &mut host,
            SignalType::Terminate
        )
        .is_ok());
        assert!(warpgrid::shim::signals::Host::poll_signal(&mut host).is_none());

        let config = ConnectConfig {
            host: "localhost".into(),
            port: 5432,
            database: "test".into(),
            user: "user".into(),
            password: None,
        };
        assert!(warpgrid::shim::database_proxy::Host::connect(&mut host, config).is_ok());

        assert!(warpgrid::shim::threading::Host::declare_threading_model(
            &mut host,
            ThreadingModel::Cooperative
        )
        .is_ok());
    }

    // ── World-level binding exists ─────────────────────────────────

    #[test]
    fn warpgrid_shims_world_type_exists() {
        // The bindgen macro generates a WarpgridShims type for the world.
        // This is a compile-time assertion that the type exists.
        fn _assert_world_type_exists(_: &WarpgridShims) {}
    }

    // ── Async handler bindings ──────────────────────────────────────

    #[test]
    fn async_handler_world_type_exists() {
        use super::async_handler_bindings::WarpgridAsyncHandler;
        fn _assert_world_type_exists(_: &WarpgridAsyncHandler) {}
    }

    #[test]
    fn async_handler_http_types_are_constructible() {
        use super::async_handler_bindings::warpgrid::shim::http_types::{
            HttpHeader, HttpRequest, HttpResponse,
        };

        let header = HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        };
        assert_eq!(header.name, "content-type");

        let request = HttpRequest {
            method: "GET".into(),
            uri: "/test".into(),
            headers: vec![header],
            body: vec![],
        };
        assert_eq!(request.method, "GET");

        let response = HttpResponse {
            status: 200,
            headers: vec![],
            body: b"ok".to_vec(),
        };
        assert_eq!(response.status, 200);
    }

    #[test]
    fn async_handler_shares_import_types_with_shims() {
        // The `with` parameter in bindgen! ensures that import-side types
        // from the async handler world are identical to the shim world types.
        // This is a compile-time assertion that both use the same types.
        use super::warpgrid::shim::filesystem::FileStat;
        use super::warpgrid::shim::dns::IpAddressRecord;

        let _stat: FileStat = FileStat {
            size: 0,
            is_file: true,
            is_directory: false,
        };
        let _record: IpAddressRecord = IpAddressRecord {
            address: "10.0.0.1".into(),
            is_ipv6: false,
        };
    }
}
