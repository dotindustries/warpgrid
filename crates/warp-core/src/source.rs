//! Multi-protocol source URI resolution.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SourceUri {
    /// OCI registry: oci://registry.example.com/my-api:v1.0.0
    Oci { registry: String, repository: String, tag: String },
    /// HTTPS: https://releases.example.com/my-api.wasm
    Https { url: String },
    /// S3: s3://bucket/path/to/module.wasm
    S3 { bucket: String, key: String },
    /// Git: git://github.com/org/repo.git#ref
    Git { url: String, reference: String },
    /// Local file: file:///path/to/module.wasm or ./relative/path.wasm
    File { path: String },
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("unsupported source scheme: {0}")]
    UnsupportedScheme(String),
    #[error("invalid source URI: {0}")]
    InvalidUri(String),
}

impl SourceUri {
    pub fn parse(uri: &str) -> Result<Self, SourceError> {
        if uri.starts_with("oci://") {
            let rest = &uri[6..];
            let (repo_path, tag) = rest.rsplit_once(':')
                .unwrap_or((rest, "latest"));
            let (registry, repository) = repo_path.split_once('/')
                .ok_or_else(|| SourceError::InvalidUri(uri.to_string()))?;
            Ok(SourceUri::Oci {
                registry: registry.to_string(),
                repository: repository.to_string(),
                tag: tag.to_string(),
            })
        } else if uri.starts_with("https://") || uri.starts_with("http://") {
            Ok(SourceUri::Https { url: uri.to_string() })
        } else if uri.starts_with("s3://") {
            let rest = &uri[5..];
            let (bucket, key) = rest.split_once('/')
                .ok_or_else(|| SourceError::InvalidUri(uri.to_string()))?;
            Ok(SourceUri::S3 {
                bucket: bucket.to_string(),
                key: key.to_string(),
            })
        } else if uri.starts_with("git://") {
            let (url, reference) = uri.rsplit_once('#')
                .unwrap_or((uri, "main"));
            Ok(SourceUri::Git {
                url: url.to_string(),
                reference: reference.to_string(),
            })
        } else if uri.starts_with("file://") {
            Ok(SourceUri::File { path: uri[7..].to_string() })
        } else if uri.starts_with("./") || uri.starts_with('/') || uri.ends_with(".wasm") {
            Ok(SourceUri::File { path: uri.to_string() })
        } else {
            Err(SourceError::UnsupportedScheme(uri.to_string()))
        }
    }

    pub fn scheme(&self) -> &'static str {
        match self {
            SourceUri::Oci { .. } => "oci",
            SourceUri::Https { .. } => "https",
            SourceUri::S3 { .. } => "s3",
            SourceUri::Git { .. } => "git",
            SourceUri::File { .. } => "file",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_oci() {
        let uri = SourceUri::parse("oci://registry.example.com/my-api:v1.0.0").unwrap();
        assert_eq!(uri.scheme(), "oci");
    }

    #[test]
    fn test_parse_https() {
        let uri = SourceUri::parse("https://cdn.example.com/app.wasm").unwrap();
        assert_eq!(uri.scheme(), "https");
    }

    #[test]
    fn test_parse_s3() {
        let uri = SourceUri::parse("s3://my-bucket/deploy/app.wasm").unwrap();
        assert_eq!(uri.scheme(), "s3");
    }

    #[test]
    fn test_parse_local_relative() {
        let uri = SourceUri::parse("./target/app.wasm").unwrap();
        assert_eq!(uri.scheme(), "file");
    }
}
