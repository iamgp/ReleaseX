pub mod cfg;
pub mod python;
pub mod toml;

use std::path::Path;

use anyhow::Result;

pub fn read_key(path: &Path, key: &str) -> Result<Option<String>> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::read_key(path, key),
        Some("cfg") => cfg::read_key(path, key),
        _ => Ok(None),
    }
}

pub fn rewrite_key(path: &Path, key: &str, version: &str) -> Result<()> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::rewrite_key(path, key, version),
        Some("cfg") => cfg::rewrite_key(path, key, version),
        _ => Ok(()),
    }
}

pub fn read_pattern(path: &Path, pattern: &str) -> Result<Option<String>> {
    python::read_pattern(path, pattern)
}

pub fn rewrite_pattern(path: &Path, pattern: &str, version: &str) -> Result<()> {
    python::rewrite_pattern(path, pattern, version)
}
