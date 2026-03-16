use std::{fs, path::Path};

use anyhow::{Context, Result, bail};

pub fn read_pattern(path: &Path, pattern: &str) -> Result<Option<String>> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let (prefix, suffix) = pattern_parts(pattern)?;

    Ok(contents
        .lines()
        .find_map(|line| extract_version(line, prefix, suffix)))
}

pub fn rewrite_pattern(path: &Path, pattern: &str, version: &str) -> Result<()> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let (prefix, suffix) = pattern_parts(pattern)?;
    let mut replaced = false;
    let updated = contents
        .lines()
        .map(|line| {
            if replaced {
                return line.to_string();
            }

            if extract_version(line, prefix, suffix).is_some() {
                replaced = true;
                let indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
                format!("{}{}{}{}", " ".repeat(indent), prefix, version, suffix)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if !replaced {
        bail!("pattern not found in {}", path.display());
    }

    let mut final_contents = updated;
    if contents.ends_with('\n') {
        final_contents.push('\n');
    }
    fs::write(path, final_contents).with_context(|| format!("failed to write {}", path.display()))
}

fn pattern_parts(pattern: &str) -> Result<(&str, &str)> {
    pattern
        .split_once("{version}")
        .ok_or_else(|| anyhow::anyhow!("pattern must contain {{version}} placeholder"))
}

fn extract_version<'a>(line: &'a str, prefix: &str, suffix: &str) -> Option<String> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix(prefix)?;
    let version = rest.strip_suffix(suffix)?;
    Some(version.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{read_pattern, rewrite_pattern};

    #[test]
    fn reads_and_rewrites_version_pattern() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("__init__.py");
        fs::write(&path, "__version__ = \"0.1.0\"\n").expect("write python file");

        let version =
            read_pattern(&path, "__version__ = \"{version}\"").expect("read version pattern");
        assert_eq!(version.as_deref(), Some("0.1.0"));

        rewrite_pattern(&path, "__version__ = \"{version}\"", "0.2.0")
            .expect("rewrite version pattern");
        let contents = fs::read_to_string(path).expect("read updated python file");
        assert_eq!(contents, "__version__ = \"0.2.0\"\n");
    }

    #[test]
    fn preserves_indentation_when_rewriting() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("__init__.py");
        fs::write(&path, "if True:\n    __version__ = '0.1.0'\n").expect("write python file");

        rewrite_pattern(&path, "__version__ = '{version}'", "0.2.0")
            .expect("rewrite version pattern");
        let contents = fs::read_to_string(path).expect("read updated python file");
        assert!(contents.contains("    __version__ = '0.2.0'"));
    }
}
