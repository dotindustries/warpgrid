//! Dockerfile static analysis (Layer 1).

use anyhow::{Result, bail};
use regex::Regex;
use std::path::Path;

/// Attempt to detect the project language from a Dockerfile.
pub fn detect_language_from_dockerfile(dockerfile: &Path) -> Result<String> {
    let content = std::fs::read_to_string(dockerfile)?;
    let from_re = Regex::new(r"(?i)^FROM\s+(\S+)")?;

    for line in content.lines() {
        if let Some(caps) = from_re.captures(line.trim()) {
            let image = caps[1].to_lowercase();
            if image.contains("rust") {
                return Ok("rust".to_string());
            } else if image.contains("golang") || image.contains("go:") {
                return Ok("go".to_string());
            } else if image.contains("node") || image.contains("bun") || image.contains("deno") {
                return Ok("typescript".to_string());
            } else if image.contains("python") {
                return Ok("python".to_string());
            }
        }
    }

    bail!("Could not detect language from Dockerfile base images")
}

/// Extract metadata from a Dockerfile.
pub struct DockerfileInfo {
    pub base_images: Vec<String>,
    pub exposed_ports: Vec<u16>,
    pub apt_packages: Vec<String>,
    pub entrypoint: Option<String>,
}

pub fn parse_dockerfile(path: &Path) -> Result<DockerfileInfo> {
    let content = std::fs::read_to_string(path)?;
    let from_re = Regex::new(r"(?i)^FROM\s+(\S+)")?;
    let expose_re = Regex::new(r"(?i)^EXPOSE\s+(\d+)")?;
    let apt_re = Regex::new(r"apt-get\s+install\s+.*?(\S+(?:\s+\S+)*)")?;
    let entry_re = Regex::new(r#"(?i)^(?:ENTRYPOINT|CMD)\s+(.+)"#)?;

    let mut info = DockerfileInfo {
        base_images: vec![],
        exposed_ports: vec![],
        apt_packages: vec![],
        entrypoint: None,
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(caps) = from_re.captures(trimmed) {
            info.base_images.push(caps[1].to_string());
        }
        if let Some(caps) = expose_re.captures(trimmed) {
            if let Ok(port) = caps[1].parse() {
                info.exposed_ports.push(port);
            }
        }
        if let Some(caps) = entry_re.captures(trimmed) {
            info.entrypoint = Some(caps[1].to_string());
        }
    }

    Ok(info)
}
