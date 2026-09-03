use serde::{Deserialize, Serialize};

use crate::{analysis::ReleaseAnalysis, config::Ecosystem};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseWorkspacePlan {
    pub schema_version: u32,
    pub ecosystem: String,
    pub release_mode: String,
    pub base_branch: String,
    pub packages: Vec<WorkspacePackagePlan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspacePackagePlan {
    pub name: String,
    pub path: String,
    pub selected: bool,
    pub selection_reason: String,
    pub current_version: String,
    pub next_version: Option<String>,
}

impl ReleaseWorkspacePlan {
    pub fn from_analysis(
        analysis: &ReleaseAnalysis,
        ecosystem: Option<Ecosystem>,
        base_branch: String,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            ecosystem: match ecosystem {
                Some(Ecosystem::Python) => "python",
                Some(Ecosystem::Rust) => "rust",
                Some(Ecosystem::Go) => "go",
                Some(Ecosystem::TypeScript) => "typescript",
                None => "unknown",
            }
            .to_string(),
            release_mode: analysis.package_plan.release_mode.clone(),
            base_branch,
            packages: analysis
                .package_plan
                .packages
                .iter()
                .map(|package| WorkspacePackagePlan {
                    name: package.name.clone(),
                    path: package.root.clone(),
                    selected: package.selected,
                    selection_reason: package.selection_reason.clone(),
                    current_version: package.current_version.to_string(),
                    next_version: package.next_version.as_ref().map(ToString::to_string),
                })
                .collect(),
        }
    }
}
