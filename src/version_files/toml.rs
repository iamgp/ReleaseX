use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

pub fn read_key(path: &Path, key: &str) -> Result<Option<String>> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value = contents
        .parse::<toml::Table>()
        .with_context(|| format!("failed to parse {}", path.display()))?;

    Ok(get_table_value(&value, key).map(|value| value.to_string()))
}

pub fn rewrite_key(path: &Path, key: &str, version: &str) -> Result<()> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut value = contents
        .parse::<toml::Table>()
        .with_context(|| format!("failed to parse {}", path.display()))?;

    let updated = set_table_value(&mut value, key, version)?;
    if !updated {
        bail!("key {key} not found in {}", path.display());
    }

    fs::write(path, toml::to_string_pretty(&value)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn get_table_value(table: &toml::Table, key: &str) -> Option<String> {
    let mut current = table;
    let mut parts = key.split('.').peekable();

    while let Some(part) = parts.next() {
        let value = current.get(part)?;
        if parts.peek().is_none() {
            return value.as_str().map(str::to_string);
        }
        current = value.as_table()?;
    }

    None
}

fn set_table_value(table: &mut toml::Table, key: &str, version: &str) -> Result<bool> {
    let parts = key.split('.').collect::<Vec<_>>();
    let Some((last, parents)) = parts.split_last() else {
        return Ok(false);
    };
    let mut current = table;

    for part in parents {
        let Some(value) = current.get_mut(*part) else {
            return Ok(false);
        };
        let Some(next) = value.as_table_mut() else {
            bail!("key path {key} does not resolve to a table");
        };
        current = next;
    }

    let Some(value) = current.get_mut(*last) else {
        return Ok(false);
    };

    *value = toml::Value::String(version.to_string());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{read_key, rewrite_key};

    #[test]
    fn reads_and_rewrites_toml_key() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("pyproject.toml");
        fs::write(&path, "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n")
            .expect("write toml file");

        let version = read_key(&path, "project.version").expect("read toml version");
        assert_eq!(version.as_deref(), Some("0.1.0"));

        rewrite_key(&path, "project.version", "0.2.0").expect("rewrite toml version");
        let version = read_key(&path, "project.version").expect("read updated version");
        assert_eq!(version.as_deref(), Some("0.2.0"));
    }
}
