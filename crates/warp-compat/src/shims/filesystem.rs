//! Filesystem shim — virtual /dev/urandom, /etc/resolv.conf, timezone data.

pub struct FilesystemShim;

impl Default for FilesystemShim {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesystemShim {
    pub fn new() -> Self {
        Self
    }
}
