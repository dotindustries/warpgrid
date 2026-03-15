//! DNS resolution shim — intercepts getaddrinfo, routes to WarpGrid service discovery.

pub struct DnsShim;

impl Default for DnsShim {
    fn default() -> Self {
        Self::new()
    }
}

impl DnsShim {
    pub fn new() -> Self {
        Self
    }
}
