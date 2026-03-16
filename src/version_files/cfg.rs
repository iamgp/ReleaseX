use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

pub fn read_key(path: &Path, key: &str) -> Result<Option<String>> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let (section, name) = split_key(key)?;
    let mut current_section = String::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].trim().to_string();
            continue;
        }

        if current_section == section {
            if let Some((candidate, value)) = trimmed.split_once('=') {
                if candidate.trim() == name {
                    return Ok(Some(value.trim().to_string()));
                }
            }
        }
    }

    Ok(None)
}

pub fn rewrite_key(path: &Path, key: &str, version: &str) -> Result<()> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let (section, name) = split_key(key)?;
    let mut current_section = String::new();
    let mut replaced = false;
    let updated = contents
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                current_section = trimmed[1..trimmed.len() - 1].trim().to_string();
                return line.to_string();
            }

            if !replaced && current_section == section {
                if let Some((candidate, _)) = trimmed.split_once('=') {
                    if candidate.trim() == name {
                        replaced = true;
                        let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
                        return format!("{}{} = {}", " ".repeat(indent), name, version);
                    }
                }
            }

            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    if !replaced {
        bail!("key {key} not found in {}", path.display());
    }

    let mut final_contents = updated;
    if contents.ends_with('\n') {
        final_contents.push('\n');
    }
    fs::write(path, final_contents).with_context(|| format!("failed to write {}", path.display()))
}

fn split_key(key: &str) -> Result<(String, String)> {
    let Some((section, name)) = key.split_once('.') else {
        bail!("cfg key must be section.name");
    };
    Ok((section.to_string(), name.to_string()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{read_key, rewrite_key};

    #[test]
    fn reads_and_rewrites_cfg_key() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("setup.cfg");
        fs::write(&path, "[metadata]\nname = demo\nversion = 0.1.0\n").expect("write cfg file");

        let version = read_key(&path, "metadata.version").expect("read cfg version");
        assert_eq!(version.as_deref(), Some("0.1.0"));

        rewrite_key(&path, "metadata.version", "0.2.0").expect("rewrite cfg version");
        let version = read_key(&path, "metadata.version").expect("read updated version");
        assert_eq!(version.as_deref(), Some("0.2.0"));
    }
}
