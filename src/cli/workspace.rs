use anyhow::{Context, Result};
use console::style;

use crate::{
    analysis::{discover_uv_workspace, extract_dependency_names, read_current_version},
    cli::Cli,
    config::Config,
};

pub fn run(cli: &Cli) -> Result<()> {
    let config = Config::load(&cli.config_path())?;
    let repo_root = std::env::current_dir().context("failed to get current directory")?;

    let (member_roots, source) = resolve_workspace_members(&repo_root, &config);

    println!();
    println!("{}", style("relx workspace").bold());
    println!();
    println!(" {} pyproject.toml", style("Workspace root:").cyan().bold());
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
        let name = detect_name(&repo_root, root);
        let version = detect_version(&repo_root, root);
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

    (Vec::new(), "none".to_string())
}

fn detect_name(repo_root: &std::path::Path, package_root: &str) -> String {
    let pyproject = repo_root.join(package_root).join("pyproject.toml");
    let contents = match std::fs::read_to_string(pyproject) {
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
        .get("project")
        .and_then(|v| v.as_table())
        .and_then(|t| t.get("name"))
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

fn detect_version(repo_root: &std::path::Path, package_root: &str) -> String {
    let version_file = crate::config::VersionFileConfig {
        path: format!("{}/pyproject.toml", package_root),
        key: Some("project.version".to_string()),
        pattern: None,
    };

    match read_current_version(repo_root, &[version_file]) {
        Ok(Some(v)) => v,
        _ => "unknown".to_string(),
    }
}
