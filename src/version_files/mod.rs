pub mod cfg;
pub mod json;
pub mod python;
pub mod toml;

use std::path::Path;

use anyhow::Result;

pub fn read_key(path: &Path, key: &str) -> Result<Option<String>> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::read_key(path, key),
        Some("cfg") => cfg::read_key(path, key),
        Some("json") => json::read_key(path, key),
        _ => Ok(None),
    }
}

pub fn read_key_from_contents(path: &str, contents: &str, key: &str) -> Result<Option<String>> {
    match Path::new(path).extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::read_key_from_contents(contents, key),
        Some("cfg") => cfg::read_key_from_contents(contents, key),
        Some("json") => json::read_key_from_contents(contents, key),
        _ => Ok(None),
    }
}

pub fn read_pattern_from_contents(contents: &str, pattern: &str) -> Result<Option<String>> {
    python::read_pattern_from_contents(contents, pattern)
}

pub fn rewrite_key(path: &Path, key: &str, version: &str) -> Result<()> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("toml") => toml::rewrite_key(path, key, version),
        Some("cfg") => cfg::rewrite_key(path, key, version),
        Some("json") => json::rewrite_key(path, key, version),
        _ => Ok(()),
    }
}

pub fn read_pattern(path: &Path, pattern: &str) -> Result<Option<String>> {
    python::read_pattern(path, pattern)
}

pub fn rewrite_pattern(path: &Path, pattern: &str, version: &str) -> Result<()> {
    python::rewrite_pattern(path, pattern, version)
}
