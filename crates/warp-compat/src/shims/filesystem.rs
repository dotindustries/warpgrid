//! Filesystem shim — virtual /dev/urandom, /etc/resolv.conf, timezone data.

pub struct FilesystemShim;

impl FilesystemShim {
    pub fn new() -> Self { Self }
}
