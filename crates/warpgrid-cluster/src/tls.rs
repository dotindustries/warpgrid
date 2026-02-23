//! mTLS certificate management.
//!
//! Generates self-signed CA and node certificates for mutual TLS
//! authentication between cluster nodes.

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use tracing::info;

/// A generated certificate and private key pair.
#[derive(Debug, Clone)]
pub struct CertKeyPair {
    /// PEM-encoded certificate.
    pub cert_pem: String,
    /// PEM-encoded private key.
    pub key_pem: String,
}

/// Generate a self-signed CA certificate for the cluster.
///
/// This CA is used to sign node certificates for mTLS.
pub fn generate_ca() -> anyhow::Result<(CertKeyPair, rcgen::Certificate)> {
    let mut params = CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);

    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "WarpGrid");
    dn.push(DnType::CommonName, "WarpGrid Cluster CA");
    params.distinguished_name = dn;

    // Valid for 10 years.
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);

    let key_pair = KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    info!("generated cluster CA certificate");

    Ok((
        CertKeyPair {
            cert_pem: cert.pem(),
            key_pem: key_pair.serialize_pem(),
        },
        cert,
    ))
}

/// Generate a node certificate signed by the cluster CA.
pub fn generate_node_cert(
    ca_key: &KeyPair,
    ca_cert: &rcgen::Certificate,
    node_id: &str,
    addresses: &[String],
) -> anyhow::Result<CertKeyPair> {
    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::OrganizationName, "WarpGrid");
    dn.push(DnType::CommonName, node_id);
    params.distinguished_name = dn;

    // Add IP SANs for the node addresses.
    for addr in addresses {
        if let Ok(ip) = addr.parse::<std::net::IpAddr>() {
            params.subject_alt_names.push(
                rcgen::SanType::IpAddress(ip),
            );
        } else {
            params.subject_alt_names.push(
                rcgen::SanType::DnsName(addr.clone().try_into()?),
            );
        }
    }

    // Valid for 1 year.
    params.not_after = rcgen::date_time_ymd(2027, 1, 1);

    let node_key = KeyPair::generate()?;
    let node_cert = params.signed_by(&node_key, ca_cert, ca_key)?;

    info!(%node_id, sans = addresses.len(), "generated node certificate");

    Ok(CertKeyPair {
        cert_pem: node_cert.pem(),
        key_pem: node_key.serialize_pem(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_ca_succeeds() {
        let (pair, _cert) = generate_ca().unwrap();
        assert!(pair.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(pair.key_pem.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn generate_node_cert_succeeds() {
        let ca_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_cert = params.self_signed(&ca_key).unwrap();

        let node_pair = generate_node_cert(
            &ca_key,
            &ca_cert,
            "node-1",
            &["10.0.0.1".to_string(), "node1.warpgrid.local".to_string()],
        )
        .unwrap();

        assert!(node_pair.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(node_pair.key_pem.contains("BEGIN PRIVATE KEY"));
    }

    #[test]
    fn generate_node_cert_with_ip_only() {
        let ca_key = KeyPair::generate().unwrap();
        let mut params = CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_cert = params.self_signed(&ca_key).unwrap();

        let node_pair = generate_node_cert(
            &ca_key,
            &ca_cert,
            "node-2",
            &["192.168.1.100".to_string()],
        )
        .unwrap();

        assert!(node_pair.cert_pem.contains("BEGIN CERTIFICATE"));
    }

    #[test]
    fn ca_and_node_certs_are_different() {
        let (ca_pair, _ca_cert) = generate_ca().unwrap();
        let ca_key = KeyPair::generate().unwrap();

        // Re-generate CA with known key for signing.
        let mut params = CertificateParams::default();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_cert = params.self_signed(&ca_key).unwrap();

        let node_pair = generate_node_cert(
            &ca_key,
            &ca_cert,
            "node-1",
            &["10.0.0.1".to_string()],
        )
        .unwrap();

        assert_ne!(ca_pair.cert_pem, node_pair.cert_pem);
    }
}
