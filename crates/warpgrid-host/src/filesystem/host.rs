//! Filesystem intercept host functions.
//!
//! Implements the `warpgrid:shim/filesystem` [`Host`] trait, intercepting
//! file operations against the [`VirtualFileMap`]. Paths matching virtual
//! entries return WarpGrid-controlled content; non-matching paths receive
//! an error, signaling the guest to fall through to the real WASI filesystem.
//!
//! # Intercept flow
//!
//! ```text
//! Guest calls open_virtual("/etc/resolv.conf")
//!   → FilesystemHost checks VirtualFileMap
//!     → Match found → allocate handle, buffer content → Ok(handle)
//!     → No match   → Err("not a virtual path") → guest falls through to WASI FS
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use crate::bindings::warpgrid::shim::filesystem::{FileStat, Host};
use super::{VirtualContent, VirtualFileMap};

/// Distinguishes special virtual files from regular buffered content.
#[derive(Debug)]
enum OpenFileKind {
    /// Regular file with buffered content — reads advance a cursor.
    Regular,
    /// `/dev/null` — reads return empty, writes are discarded.
    DevNull,
    /// `/dev/urandom` — each read generates fresh random bytes.
    DevUrandom,
}

/// State for a single open virtual file handle.
#[derive(Debug)]
struct OpenVirtualFile {
    /// Buffered content (empty for DevNull and DevUrandom).
    content: Vec<u8>,
    /// Current read position within `content` (unused for DevNull/DevUrandom).
    position: usize,
    /// What kind of virtual file this is.
    kind: OpenFileKind,
}

/// Host-side implementation of the `warpgrid:shim/filesystem` interface.
///
/// Intercepts file operations by checking paths against the [`VirtualFileMap`].
/// For paths that match a virtual entry, content is served from memory.
/// For paths that don't match, an error is returned to signal the guest
/// to fall through to the standard WASI filesystem implementation.
///
/// All intercept decisions are logged at `tracing::debug` level.
pub struct FilesystemHost {
    /// Immutable virtual file map (shared across instances).
    file_map: Arc<VirtualFileMap>,
    /// Open file handles → file state.
    open_files: HashMap<u64, OpenVirtualFile>,
    /// Next handle to allocate (monotonically increasing, starts at 1).
    next_handle: u64,
}

impl FilesystemHost {
    /// Create a new `FilesystemHost` backed by the given virtual file map.
    pub fn new(file_map: Arc<VirtualFileMap>) -> Self {
        Self {
            file_map,
            open_files: HashMap::new(),
            next_handle: 1,
        }
    }

    /// Allocate the next file handle.
    fn allocate_handle(&mut self) -> u64 {
        let handle = self.next_handle;
        self.next_handle += 1;
        handle
    }
}

impl Host for FilesystemHost {
    fn open_virtual(&mut self, path: String) -> Result<u64, String> {
        tracing::debug!(path = %path, "filesystem intercept: open_virtual");

        let content = self.file_map.lookup(&path);

        match content {
            VirtualContent::Found(data) => {
                let handle = self.allocate_handle();
                tracing::debug!(
                    path = %path,
                    handle = handle,
                    size = data.len(),
                    "virtual path matched — opened regular file"
                );
                self.open_files.insert(
                    handle,
                    OpenVirtualFile {
                        content: data,
                        position: 0,
                        kind: OpenFileKind::Regular,
                    },
                );
                Ok(handle)
            }
            VirtualContent::DevNull => {
                let handle = self.allocate_handle();
                tracing::debug!(
                    path = %path,
                    handle = handle,
                    "virtual path matched — opened /dev/null"
                );
                self.open_files.insert(
                    handle,
                    OpenVirtualFile {
                        content: Vec::new(),
                        position: 0,
                        kind: OpenFileKind::DevNull,
                    },
                );
                Ok(handle)
            }
            VirtualContent::DevUrandom => {
                let handle = self.allocate_handle();
                tracing::debug!(
                    path = %path,
                    handle = handle,
                    "virtual path matched — opened /dev/urandom"
                );
                self.open_files.insert(
                    handle,
                    OpenVirtualFile {
                        content: Vec::new(),
                        position: 0,
                        kind: OpenFileKind::DevUrandom,
                    },
                );
                Ok(handle)
            }
            VirtualContent::NotFound => {
                tracing::debug!(
                    path = %path,
                    "virtual path not matched — fall through to WASI filesystem"
                );
                Err(format!("not a virtual path: {path}"))
            }
        }
    }

    fn read_virtual(&mut self, handle: u64, len: u32) -> Result<Vec<u8>, String> {
        let file = self
            .open_files
            .get_mut(&handle)
            .ok_or_else(|| format!("invalid handle: {handle}"))?;

        let len = len as usize;

        match file.kind {
            OpenFileKind::DevNull => {
                tracing::debug!(handle = handle, "read /dev/null — returning empty");
                Ok(Vec::new())
            }
            OpenFileKind::DevUrandom => {
                let mut buf = vec![0u8; len];
                getrandom::getrandom(&mut buf)
                    .map_err(|e| format!("getrandom failed: {e}"))?;
                tracing::debug!(
                    handle = handle,
                    bytes = len,
                    "read /dev/urandom — returning random bytes"
                );
                Ok(buf)
            }
            OpenFileKind::Regular => {
                let remaining = file.content.len().saturating_sub(file.position);
                let to_read = len.min(remaining);
                let data = file.content[file.position..file.position + to_read].to_vec();
                file.position += to_read;
                tracing::debug!(
                    handle = handle,
                    bytes_read = to_read,
                    position = file.position,
                    remaining = remaining - to_read,
                    "read virtual file"
                );
                Ok(data)
            }
        }
    }

    fn stat_virtual(&mut self, path: String) -> Result<FileStat, String> {
        tracing::debug!(path = %path, "filesystem intercept: stat_virtual");

        let content = self.file_map.lookup(&path);

        match content {
            VirtualContent::Found(data) => {
                tracing::debug!(path = %path, size = data.len(), "stat virtual file");
                Ok(FileStat {
                    size: data.len() as u64,
                    is_file: true,
                    is_directory: false,
                })
            }
            VirtualContent::DevNull | VirtualContent::DevUrandom => {
                tracing::debug!(path = %path, "stat character device");
                Ok(FileStat {
                    size: 0,
                    is_file: false,
                    is_directory: false,
                })
            }
            VirtualContent::NotFound => {
                tracing::debug!(path = %path, "stat not matched — fall through");
                Err(format!("not a virtual path: {path}"))
            }
        }
    }

    fn close_virtual(&mut self, handle: u64) -> Result<(), String> {
        match self.open_files.remove(&handle) {
            Some(_) => {
                tracing::debug!(handle = handle, "closed virtual file handle");
                Ok(())
            }
            None => {
                tracing::debug!(handle = handle, "close failed — invalid handle");
                Err(format!("invalid handle: {handle}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a `FilesystemHost` with default virtual paths.
    fn default_host() -> FilesystemHost {
        FilesystemHost::new(Arc::new(VirtualFileMap::with_defaults()))
    }

    /// Create a `FilesystemHost` with a custom file map.
    fn host_with_map(map: VirtualFileMap) -> FilesystemHost {
        FilesystemHost::new(Arc::new(map))
    }

    // ── open_virtual ─────────────────────────────────────────────────

    #[test]
    fn open_known_path_returns_handle() {
        let mut host = default_host();
        let handle = host.open_virtual("/etc/resolv.conf".into());
        assert!(handle.is_ok());
        assert!(handle.unwrap() > 0);
    }

    #[test]
    fn open_dev_null_returns_handle() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/null".into());
        assert!(handle.is_ok());
    }

    #[test]
    fn open_dev_urandom_returns_handle() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/urandom".into());
        assert!(handle.is_ok());
    }

    #[test]
    fn open_unknown_path_returns_error() {
        let mut host = default_host();
        let result = host.open_virtual("/tmp/nonexistent".into());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a virtual path"));
    }

    #[test]
    fn open_with_path_traversal_matches() {
        let mut host = default_host();
        let handle = host.open_virtual("/etc/../etc/hosts".into());
        assert!(handle.is_ok());
    }

    #[test]
    fn open_same_path_twice_returns_different_handles() {
        let mut host = default_host();
        let h1 = host.open_virtual("/etc/hosts".into()).unwrap();
        let h2 = host.open_virtual("/etc/hosts".into()).unwrap();
        assert_ne!(h1, h2);
    }

    #[test]
    fn handles_are_monotonically_increasing() {
        let mut host = default_host();
        let h1 = host.open_virtual("/etc/hosts".into()).unwrap();
        let h2 = host.open_virtual("/etc/resolv.conf".into()).unwrap();
        let h3 = host.open_virtual("/dev/null".into()).unwrap();
        assert!(h1 < h2);
        assert!(h2 < h3);
    }

    #[test]
    fn open_prefix_path_returns_handle() {
        let mut host = default_host();
        let handle = host.open_virtual("/usr/share/zoneinfo/UTC".into());
        assert!(handle.is_ok());
    }

    // ── read_virtual ─────────────────────────────────────────────────

    #[test]
    fn read_returns_correct_content() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("nameserver 10.0.0.1\n")
            .build();
        let mut host = host_with_map(map);
        let handle = host.open_virtual("/etc/resolv.conf".into()).unwrap();
        let data = host.read_virtual(handle, 1024).unwrap();
        assert_eq!(String::from_utf8_lossy(&data), "nameserver 10.0.0.1\n");
    }

    #[test]
    fn read_partial_content() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("nameserver 10.0.0.1\n")
            .build();
        let mut host = host_with_map(map);
        let handle = host.open_virtual("/etc/resolv.conf".into()).unwrap();
        let data = host.read_virtual(handle, 10).unwrap();
        assert_eq!(data.len(), 10);
        assert_eq!(String::from_utf8_lossy(&data), "nameserver");
    }

    #[test]
    fn sequential_reads_advance_cursor() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("ABCDEFGHIJ")
            .build();
        let mut host = host_with_map(map);
        let handle = host.open_virtual("/etc/resolv.conf".into()).unwrap();
        let part1 = host.read_virtual(handle, 5).unwrap();
        assert_eq!(part1, b"ABCDE");
        let part2 = host.read_virtual(handle, 5).unwrap();
        assert_eq!(part2, b"FGHIJ");
    }

    #[test]
    fn read_past_eof_returns_empty() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("short")
            .build();
        let mut host = host_with_map(map);
        let handle = host.open_virtual("/etc/resolv.conf".into()).unwrap();
        let _ = host.read_virtual(handle, 100).unwrap();
        let data = host.read_virtual(handle, 100).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn read_dev_null_returns_empty() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/null".into()).unwrap();
        let data = host.read_virtual(handle, 1024).unwrap();
        assert!(data.is_empty());
    }

    #[test]
    fn read_dev_null_always_empty() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/null".into()).unwrap();
        for _ in 0..5 {
            let data = host.read_virtual(handle, 100).unwrap();
            assert!(data.is_empty());
        }
    }

    #[test]
    fn read_dev_urandom_returns_requested_length() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/urandom".into()).unwrap();
        let data = host.read_virtual(handle, 32).unwrap();
        assert_eq!(data.len(), 32);
    }

    #[test]
    fn read_dev_urandom_different_bytes_each_time() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/urandom".into()).unwrap();
        let a = host.read_virtual(handle, 32).unwrap();
        let b = host.read_virtual(handle, 32).unwrap();
        // Probability of collision: 2^-256 — effectively zero.
        assert_ne!(a, b);
    }

    #[test]
    fn read_dev_urandom_bytes_not_all_zero() {
        let mut host = default_host();
        let handle = host.open_virtual("/dev/urandom".into()).unwrap();
        let data = host.read_virtual(handle, 256).unwrap();
        assert!(data.iter().any(|&b| b != 0));
    }

    #[test]
    fn read_invalid_handle_returns_error() {
        let mut host = default_host();
        let result = host.read_virtual(999, 100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid handle"));
    }

    // ── stat_virtual ─────────────────────────────────────────────────

    #[test]
    fn stat_regular_file() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("nameserver 10.0.0.1\n")
            .build();
        let mut host = host_with_map(map);
        let stat = host.stat_virtual("/etc/resolv.conf".into()).unwrap();
        assert_eq!(stat.size, 20); // "nameserver 10.0.0.1\n"
        assert!(stat.is_file);
        assert!(!stat.is_directory);
    }

    #[test]
    fn stat_dev_null() {
        let mut host = default_host();
        let stat = host.stat_virtual("/dev/null".into()).unwrap();
        assert_eq!(stat.size, 0);
        assert!(!stat.is_file); // character device
        assert!(!stat.is_directory);
    }

    #[test]
    fn stat_dev_urandom() {
        let mut host = default_host();
        let stat = host.stat_virtual("/dev/urandom".into()).unwrap();
        assert_eq!(stat.size, 0);
        assert!(!stat.is_file); // character device
        assert!(!stat.is_directory);
    }

    #[test]
    fn stat_unknown_path_returns_error() {
        let mut host = default_host();
        let result = host.stat_virtual("/nonexistent".into());
        assert!(result.is_err());
    }

    #[test]
    fn stat_with_path_canonicalization() {
        let mut host = default_host();
        let stat = host.stat_virtual("/etc/../etc/resolv.conf".into());
        assert!(stat.is_ok());
    }

    // ── close_virtual ────────────────────────────────────────────────

    #[test]
    fn close_valid_handle() {
        let mut host = default_host();
        let handle = host.open_virtual("/etc/hosts".into()).unwrap();
        assert!(host.close_virtual(handle).is_ok());
    }

    #[test]
    fn close_invalid_handle_returns_error() {
        let mut host = default_host();
        let result = host.close_virtual(999);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid handle"));
    }

    #[test]
    fn read_after_close_returns_error() {
        let mut host = default_host();
        let handle = host.open_virtual("/etc/hosts".into()).unwrap();
        host.close_virtual(handle).unwrap();
        let result = host.read_virtual(handle, 100);
        assert!(result.is_err());
    }

    #[test]
    fn close_same_handle_twice_second_fails() {
        let mut host = default_host();
        let handle = host.open_virtual("/etc/hosts".into()).unwrap();
        host.close_virtual(handle).unwrap();
        let result = host.close_virtual(handle);
        assert!(result.is_err());
    }

    // ── Full lifecycle ───────────────────────────────────────────────

    #[test]
    fn full_lifecycle_open_read_close() {
        let map = VirtualFileMap::builder()
            .with_etc_hosts("127.0.0.1 localhost\n")
            .build();
        let mut host = host_with_map(map);
        let handle = host.open_virtual("/etc/hosts".into()).unwrap();
        let data = host.read_virtual(handle, 1024).unwrap();
        assert_eq!(String::from_utf8_lossy(&data), "127.0.0.1 localhost\n");
        host.close_virtual(handle).unwrap();
    }

    #[test]
    fn multiple_files_open_simultaneously() {
        let mut host = default_host();
        let h1 = host.open_virtual("/etc/hosts".into()).unwrap();
        let h2 = host.open_virtual("/etc/resolv.conf".into()).unwrap();

        let data1 = host.read_virtual(h1, 1024).unwrap();
        let data2 = host.read_virtual(h2, 1024).unwrap();

        assert!(String::from_utf8_lossy(&data1).contains("localhost"));
        assert!(String::from_utf8_lossy(&data2).contains("nameserver"));

        host.close_virtual(h1).unwrap();
        host.close_virtual(h2).unwrap();
    }

    #[test]
    fn independent_cursors_for_same_path() {
        let map = VirtualFileMap::builder()
            .with_resolv_conf("ABCDEFGHIJ")
            .build();
        let mut host = host_with_map(map);

        let h1 = host.open_virtual("/etc/resolv.conf".into()).unwrap();
        let h2 = host.open_virtual("/etc/resolv.conf".into()).unwrap();

        // Read 5 bytes from h1, then 3 from h2 — cursors are independent.
        let d1 = host.read_virtual(h1, 5).unwrap();
        let d2 = host.read_virtual(h2, 3).unwrap();
        assert_eq!(d1, b"ABCDE");
        assert_eq!(d2, b"ABC");

        // Continue reading from h1 — picks up after "ABCDE".
        let d3 = host.read_virtual(h1, 5).unwrap();
        assert_eq!(d3, b"FGHIJ");

        host.close_virtual(h1).unwrap();
        host.close_virtual(h2).unwrap();
    }

    #[test]
    fn path_canonicalization_prevents_bypass() {
        let map = VirtualFileMap::builder()
            .with_etc_hosts("secure content\n")
            .build();
        let mut host = host_with_map(map);

        let traversal_paths = [
            "/etc/../etc/hosts",
            "/etc/./hosts",
            "/../../../etc/hosts",
            "/a/b/../../etc/hosts",
        ];

        for path in &traversal_paths {
            let handle = host.open_virtual(path.to_string()).unwrap();
            let data = host.read_virtual(handle, 1024).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&data),
                "secure content\n",
                "path traversal bypass via {path}"
            );
            host.close_virtual(handle).unwrap();
        }
    }

    #[test]
    fn prefix_path_timezone_lifecycle() {
        let mut host = default_host();
        let handle = host.open_virtual("/usr/share/zoneinfo/UTC".into()).unwrap();
        let data = host.read_virtual(handle, 4096).unwrap();
        assert!(data.starts_with(b"TZif"));
        host.close_virtual(handle).unwrap();
    }

    #[test]
    fn read_zero_bytes_returns_empty() {
        let mut host = default_host();
        let handle = host.open_virtual("/etc/hosts".into()).unwrap();
        let data = host.read_virtual(handle, 0).unwrap();
        assert!(data.is_empty());
        // Cursor should not advance — subsequent full read gets all content.
        let full = host.read_virtual(handle, 4096).unwrap();
        assert!(String::from_utf8_lossy(&full).contains("localhost"));
        host.close_virtual(handle).unwrap();
    }
}
