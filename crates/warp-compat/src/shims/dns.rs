//! DNS resolution shim — intercepts getaddrinfo, routes to WarpGrid service discovery.

pub struct DnsShim;

impl DnsShim {
    pub fn new() -> Self {
        Self
    }
}
