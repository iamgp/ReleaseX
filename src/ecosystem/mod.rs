use std::{fs, path::Path};

use crate::config::{Config, Ecosystem, VersionFileConfig};

pub fn detect(repo_root: &Path, config: Option<&Config>) -> Ecosystem {
    if let Some(ecosystem) = config.and_then(|cfg| cfg.project.ecosystem) {
        return ecosystem;
    }

    if repo_root.join("Cargo.toml").exists() {
        return Ecosystem::Rust;
    }

    if repo_root.join("go.mod").exists() {
        return Ecosystem::Go;
    }

    Ecosystem::Python
}

pub fn manifest_name(ecosystem: Ecosystem) -> &'static str {
    match ecosystem {
        Ecosystem::Python => "pyproject.toml",
        Ecosystem::Rust => "Cargo.toml",
        Ecosystem::Go => "go.mod",
    }
}

pub fn discover_version_files(repo_root: &Path, ecosystem: Ecosystem) -> Vec<VersionFileConfig> {
    match ecosystem {
        Ecosystem::Python => discover_python_version_files(repo_root),
        Ecosystem::Rust => discover_rust_version_files(repo_root),
        Ecosystem::Go => discover_go_version_files(repo_root),
    }
}

pub fn build_command(ecosystem: Ecosystem, pyproject_backend: Option<&str>) -> &'static str {
    match ecosystem {
        Ecosystem::Python => {
            if pyproject_backend.is_some_and(|backend| backend.contains("maturin")) {
                "maturin build --release"
            } else {
                "uv build"
            }
        }
        Ecosystem::Rust => "cargo build --locked",
        Ecosystem::Go => "go build ./...",
    }
}

pub fn healthcheck_command(
    ecosystem: Ecosystem,
    pyproject_backend: Option<&str>,
) -> Vec<&'static str> {
    match ecosystem {
        Ecosystem::Python => {
            if pyproject_backend.is_some_and(|backend| backend.contains("maturin")) {
                vec!["maturin", "build", "--release"]
            } else {
                vec!["uv", "build", "--no-sources"]
            }
        }
        Ecosystem::Rust => vec!["cargo", "build", "--locked"],
        Ecosystem::Go => vec!["go", "build", "./..."],
    }
}

pub fn tool_check_command(
    ecosystem: Ecosystem,
    publish_provider: Option<&str>,
) -> Vec<&'static str> {
    match ecosystem {
        Ecosystem::Python => match publish_provider.unwrap_or("uv") {
            "twine" => vec!["twine", "--version"],
            _ => vec!["uv", "--version"],
        },
        Ecosystem::Rust => vec!["cargo", "--version"],
        Ecosystem::Go => match publish_provider.unwrap_or("goreleaser") {
            "goreleaser" => vec!["goreleaser", "--version"],
            _ => vec!["go", "version"],
        },
    }
}

pub fn python_build_backend(repo_root: &Path) -> Option<String> {
    let path = repo_root.join("pyproject.toml");
    let contents = fs::read_to_string(path).ok()?;
    let table = contents.parse::<toml::Table>().ok()?;

    table
        .get("build-system")
        .and_then(|bs| bs.as_table())
        .and_then(|bs| bs.get("build-backend"))
        .and_then(|bb| bb.as_str())
        .map(ToString::to_string)
}

fn discover_python_version_files(repo_root: &Path) -> Vec<VersionFileConfig> {
    let mut version_files = Vec::new();

    let pyproject_path = repo_root.join("pyproject.toml");
    if pyproject_path.exists() {
        version_files.push(VersionFileConfig {
            path: "pyproject.toml".to_string(),
            key: Some("project.version".to_string()),
            pattern: None,
        });
    }

    let setup_cfg_path = repo_root.join("setup.cfg");
    if setup_cfg_path.exists() {
        version_files.push(VersionFileConfig {
            path: "setup.cfg".to_string(),
            key: Some("metadata.version".to_string()),
            pattern: None,
        });
    }

    version_files.extend(scan_python_version_patterns(repo_root));

    if version_files.is_empty() {
        version_files.push(VersionFileConfig {
            path: "pyproject.toml".to_string(),
            key: Some("project.version".to_string()),
            pattern: None,
        });
    }

    version_files
}

fn discover_rust_version_files(repo_root: &Path) -> Vec<VersionFileConfig> {
    let mut version_files = Vec::new();

    if repo_root.join("Cargo.toml").exists() {
        version_files.push(VersionFileConfig {
            path: "Cargo.toml".to_string(),
            key: Some("package.version".to_string()),
            pattern: None,
        });
    }

    version_files
}

fn discover_go_version_files(repo_root: &Path) -> Vec<VersionFileConfig> {
    let mut version_files = Vec::new();

    for candidate in ["VERSION", "version.txt"] {
        if repo_root.join(candidate).exists() {
            version_files.push(VersionFileConfig {
                path: candidate.to_string(),
                key: None,
                pattern: Some("{version}".to_string()),
            });
        }
    }

    scan_go_dir(repo_root, repo_root, &mut version_files);
    version_files.sort_by(|left, right| left.path.cmp(&right.path));
    version_files.dedup_by(|left, right| left.path == right.path);
    version_files
}

fn scan_python_version_patterns(repo_root: &Path) -> Vec<VersionFileConfig> {
    let mut candidates = Vec::new();

    for relative in ["src", "."] {
        let dir = repo_root.join(relative);
        if dir.is_dir() {
            scan_python_dir(repo_root, &dir, &mut candidates);
        }
    }

    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);
    candidates
}

fn scan_python_dir(repo_root: &Path, dir: &Path, candidates: &mut Vec<VersionFileConfig>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | "target" | ".venv" | "venv" | "__pycache__")
        ) {
            continue;
        }

        if path.is_dir() {
            scan_python_dir(repo_root, &path, candidates);
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) != Some("__init__.py") {
            continue;
        }

        let Some(pattern) = detect_python_pattern(&path) else {
            continue;
        };
        let Ok(relative_path) = path.strip_prefix(repo_root) else {
            continue;
        };

        candidates.push(VersionFileConfig {
            path: relative_path.to_string_lossy().replace('\\', "/"),
            key: None,
            pattern: Some(pattern),
        });
    }
}

fn scan_go_dir(repo_root: &Path, dir: &Path, candidates: &mut Vec<VersionFileConfig>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | "target" | "vendor")
        ) {
            continue;
        }

        if path.is_dir() {
            scan_go_dir(repo_root, &path, candidates);
            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("go") {
            continue;
        }

        let Some(pattern) = detect_go_pattern(&path) else {
            continue;
        };
        let Ok(relative_path) = path.strip_prefix(repo_root) else {
            continue;
        };

        candidates.push(VersionFileConfig {
            path: relative_path.to_string_lossy().replace('\\', "/"),
            key: None,
            pattern: Some(pattern),
        });
    }
}

fn detect_python_pattern(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;

    for line in contents.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("__version__") {
            continue;
        }

        let (prefix, raw_value) = trimmed.split_once('=')?;
        let value = raw_value.trim();
        if value.len() < 2 {
            continue;
        }

        let quote = value.chars().next()?;
        if (quote != '"' && quote != '\'') || !value.ends_with(quote) {
            continue;
        }

        return Some(format!("{}= {}{{version}}{}", prefix, quote, quote));
    }

    None
}

fn detect_go_pattern(path: &Path) -> Option<String> {
    let contents = fs::read_to_string(path).ok()?;

    for line in contents.lines() {
        let trimmed = line.trim();
        if !(trimmed.starts_with("const Version")
            || trimmed.starts_with("var Version")
            || trimmed.starts_with("const AppVersion")
            || trimmed.starts_with("var AppVersion"))
        {
            continue;
        }

        let (prefix, raw_value) = trimmed.split_once('=')?;
        let value = raw_value.trim();
        if value.len() < 2 {
            continue;
        }

        let quote = value.chars().next()?;
        if (quote != '"' && quote != '\'') || !value.ends_with(quote) {
            continue;
        }

        return Some(format!("{}= {}{{version}}{}", prefix, quote, quote));
    }

    None
}
