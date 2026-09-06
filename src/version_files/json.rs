use std::fs;

use anyhow::{Context, Result};

fn read_document(path: &std::path::Path) -> Result<serde_json::Value> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

fn lookup<'a>(document: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    let mut current = document;
    for segment in key.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

pub fn read_key_from_contents(contents: &str, key: &str) -> Result<Option<String>> {
    let document: serde_json::Value =
        serde_json::from_str(contents).context("failed to parse json")?;
    match lookup(&document, key) {
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Ok(Some(
            other.as_str().unwrap_or(&other.to_string()).to_string(),
        )),
        None => Ok(None),
    }
}

pub fn read_key(path: &std::path::Path, key: &str) -> Result<Option<String>> {
    let document = read_document(path)?;
    match lookup(&document, key) {
        Some(serde_json::Value::String(value)) => Ok(Some(value.clone())),
        Some(other) => Ok(Some(
            other.as_str().unwrap_or(&other.to_string()).to_string(),
        )),
        None => Ok(None),
    }
}

pub fn rewrite_key(path: &std::path::Path, key: &str, version: &str) -> Result<()> {
    let mut document = read_document(path)?;
    let mut current = &mut document;
    let mut segments = key.split('.').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            current[segment] = serde_json::Value::String(version.to_string());
        } else {
            if !current.get(segment).is_some_and(|value| value.is_object()) {
                current[segment] = serde_json::Value::Object(serde_json::Map::new());
            }
            current = &mut current[segment];
        }
    }

    let mut rendered = serde_json::to_string_pretty(&document)?;
    rendered.push('\n');
    fs::write(path, rendered).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{read_key, rewrite_key};

    #[test]
    fn round_trips_package_json_version() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("package.json");
        fs::write(
            &path,
            "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n",
        )
        .expect("write");

        assert_eq!(
            read_key(&path, "version").expect("read"),
            Some("0.1.0".to_string())
        );
        rewrite_key(&path, "version", "0.2.0").expect("rewrite");
        assert_eq!(
            read_key(&path, "version").expect("read"),
            Some("0.2.0".to_string())
        );
        assert_eq!(
            read_key(&path, "name").expect("read"),
            Some("demo".to_string())
        );
    }
}
