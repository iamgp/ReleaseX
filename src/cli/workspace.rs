use anyhow::{Context, Result};
use console::style;

use crate::{
    analysis::{
        discover_cargo_workspace, discover_go_workspace, discover_uv_workspace,
        extract_dependency_names, read_current_version,
    },
    cli::Cli,
    config::{Config, Ecosystem},
    ecosystem,
};

pub fn run(cli: &Cli) -> Result<()> {
    let config = Config::load(&cli.config_path())?;
    let repo_root = std::env::current_dir().context("failed to get current directory")?;
    let active_ecosystem = ecosystem::detect(&repo_root, Some(&config));

    let (member_roots, source) = resolve_workspace_members(&repo_root, &config, active_ecosystem);

    println!();
    println!("{}", style("relx workspace").bold());
    println!();
    println!(
        " {} {}",
        style("Workspace root:").cyan().bold(),
        ecosystem::manifest_name(active_ecosystem)
    );
    println!(" {} {}", style("Discovery:").cyan().bold(), source);

    if member_roots.is_empty() {
        println!();
        println!(" {}", style("No workspace members found.").dim());
        println!();
        return Ok(());
    }

    // Collect package info
    let mut members: Vec<MemberInfo> = Vec::new();
    for root in &member_roots {
        let name = detect_name(&repo_root, root, active_ecosystem);
        let version = detect_version(&repo_root, root, active_ecosystem);
        let deps = extract_dependency_names(&repo_root, root);
        members.push(MemberInfo {
            root: root.clone(),
            name,
            version,
            deps,
        });
    }

    let all_names: Vec<String> = members.iter().map(|m| m.name.clone()).collect();

    println!(" {}:", style("Members").cyan().bold());
    for member in &members {
        let internal_deps: Vec<&str> = member
            .deps
            .iter()
            .filter(|d| all_names.contains(d) && **d != member.name)
            .map(|d| {
                // Show just the short name
                d.rsplit('/').next().unwrap_or(d)
            })
            .collect();

        let dep_suffix = if internal_deps.is_empty() {
            String::new()
        } else {
            format!(" — depends on {}", internal_deps.join(", "))
        };

        println!(
            "   {} ({} {}){}",
            style(&member.root).white(),
            style(&member.name).bold(),
            member.version,
            style(dep_suffix).dim()
        );
    }
    let known_versions: std::collections::BTreeSet<&str> = members
        .iter()
        .map(|member| member.version.as_str())
        .collect();
    if known_versions.len() > 1 {
        println!();
        println!(
            " {} workspace members have mismatched versions",
            style("Warning:").yellow().bold()
        );
    }
    println!();

    Ok(())
}

struct MemberInfo {
    root: String,
    name: String,
    version: String,
    deps: Vec<String>,
}

fn resolve_workspace_members(
    repo_root: &std::path::Path,
    config: &Config,
    active_ecosystem: Ecosystem,
) -> (Vec<String>, String) {
    if !config.monorepo.packages.is_empty() {
        return (
            config.monorepo.packages.clone(),
            "[monorepo].packages".to_string(),
        );
    }

    if let Some(roots) = discover_uv_workspace(repo_root) {
        return (
            roots,
            "uv workspace (tool.uv.workspace.members)".to_string(),
        );
    }

    if active_ecosystem == Ecosystem::Rust
        && let Some(roots) = discover_cargo_workspace(repo_root)
    {
        return (roots, "cargo workspace (workspace.members)".to_string());
    }

    if active_ecosystem == Ecosystem::Go
        && let Some(roots) = discover_go_workspace(repo_root)
    {
        return (roots, "go workspace (go.work use)".to_string());
    }

    (Vec::new(), "none".to_string())
}

fn detect_name(repo_root: &std::path::Path, package_root: &str, ecosystem: Ecosystem) -> String {
    let manifest = match ecosystem {
        Ecosystem::Python => ("pyproject.toml", "project", "name"),
        Ecosystem::Rust => ("Cargo.toml", "package", "name"),
        Ecosystem::Go => ("go.mod", "", ""),
    };

    if ecosystem == Ecosystem::Go {
        let go_mod = repo_root.join(package_root).join("go.mod");
        let contents = match std::fs::read_to_string(go_mod) {
            Ok(c) => c,
            Err(_) => {
                return package_root
                    .rsplit('/')
                    .next()
                    .unwrap_or(package_root)
                    .to_string();
            }
        };
        return contents
            .lines()
            .find_map(|line| line.trim().strip_prefix("module ").map(str::trim))
            .map(|module| module.rsplit('/').next().unwrap_or(module).to_string())
            .unwrap_or_else(|| {
                package_root
                    .rsplit('/')
                    .next()
                    .unwrap_or(package_root)
                    .to_string()
            });
    }

    let file = repo_root.join(package_root).join(manifest.0);
    let contents = match std::fs::read_to_string(file) {
        Ok(c) => c,
        Err(_) => {
            return package_root
                .rsplit('/')
                .next()
                .unwrap_or(package_root)
                .to_string();
        }
    };
    let parsed = match contents.parse::<toml::Table>() {
        Ok(t) => t,
        Err(_) => {
            return package_root
                .rsplit('/')
                .next()
                .unwrap_or(package_root)
                .to_string();
        }
    };

    parsed
        .get(manifest.1)
        .and_then(|v| v.as_table())
        .and_then(|t| t.get(manifest.2))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            package_root
                .rsplit('/')
                .next()
                .unwrap_or(package_root)
                .to_string()
        })
}

fn detect_version(repo_root: &std::path::Path, package_root: &str, ecosystem: Ecosystem) -> String {
    let version_file = match ecosystem {
        Ecosystem::Python => crate::config::VersionFileConfig {
            path: format!("{}/pyproject.toml", package_root),
            key: Some("project.version".to_string()),
            pattern: None,
        },
        Ecosystem::Rust => crate::config::VersionFileConfig {
            path: format!("{}/Cargo.toml", package_root),
            key: Some("package.version".to_string()),
            pattern: None,
        },
        Ecosystem::Go => crate::config::VersionFileConfig {
            path: format!("{}/VERSION", package_root),
            key: None,
            pattern: Some("{version}".to_string()),
        },
    };

    match read_current_version(repo_root, &[version_file]) {
        Ok(Some(v)) => v,
        _ => "unknown".to_string(),
    }
}
