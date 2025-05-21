use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use tracing;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::terminal;

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PackageJsonData {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scripts: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub dependencies: HashMap<String, String>,
    #[serde(
        default,
        rename = "devDependencies",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub dev_dependencies: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

async fn ensure_dependency_internal(
    project_dir: &Path,
    package_json_data: &mut PackageJsonData,
    dep_name: &str,
    target_version: &str, 
    is_dev_dependency: bool,
) -> Result<bool> {
    let mut modified = false;
    let dep_map = if is_dev_dependency {
        &mut package_json_data.dev_dependencies
    } else {
        &mut package_json_data.dependencies
    };

    let needs_install_or_update = match dep_map.get(dep_name) {
        Some(current_version) => current_version != target_version,
        None => true,
    };

    if needs_install_or_update {
        tracing::info!(target: "dev_setup::nextjs", dependency = dep_name, version = target_version, "Ensuring dependency is installed/updated.");
        let mut install_args = vec!["install", "--loglevel", "error"];
        if is_dev_dependency {
            install_args.push("--save-dev");
        }
        let dep_with_version = format!("{}@{}", dep_name, target_version);
        install_args.push(&dep_with_version);

        terminal::npm::run_npm_command(project_dir, &install_args, false)
            .await
            .with_context(|| {
                format!(
                    "dev_setup::nextjs: Failed to install/update dependency '{}'",
                    dep_name
                )
            })?;

        dep_map.insert(dep_name.to_string(), target_version.to_string());
        modified = true;
    }
    Ok(modified)
}

fn ensure_script_internal(
    package_json_data: &mut PackageJsonData,
    script_name: &str,
    script_command: &str,
) -> bool {
    let mut modified = false;
    match package_json_data.scripts.get(script_name) {
        Some(current_command) if current_command == script_command => {}
        _ => {
            tracing::info!(target: "dev_setup::nextjs", script_name, script_command, "Ensuring npm script exists/is correct.");
            package_json_data
                .scripts
                .insert(script_name.to_string(), script_command.to_string());
            modified = true;
        }
    }
    modified
}

pub async fn ensure_project_dependencies_and_scripts(project_dir: &Path) -> Result<()> {
    let package_json_path = project_dir.join("package.json");

    if !package_json_path.exists() {
        tracing::info!(
            target: "dev_setup::nextjs",
            path = %project_dir.display(),
            "Initializing new package.json..."
        );
        terminal::npm::run_npm_command(&project_dir, &["init", "-y"], false)
            .await
            .context("dev_setup::nextjs: Failed to initialize new package.json. Please create it manually or check npm installation.")?;
        tracing::info!(target: "dev_setup::nextjs", "package.json initialized. Please review and customize if necessary.");
    }

    let content = fs::read_to_string(&package_json_path).with_context(|| {
        format!(
            "dev_setup::nextjs: Failed to read package.json from {}",
            package_json_path.display()
        )
    })?;

    let mut package_data: PackageJsonData = serde_json::from_str(&content).with_context(|| {
        format!(
            "dev_setup::nextjs: Failed to parse package.json from {}. Content: {}",
            package_json_path.display(),
            content
        )
    })?;

    let mut modified_package_json = false;

    let deps_to_ensure = [
        ("next", "15.3.2", false),
        ("react", "^19.0.0", false),
        ("react-dom", "^19.0.0", false),
        ("eslint", "^9", true),
        ("prettier", "3.5.3", true),
        ("typescript-language-server", "^4.3.4", true),
        ("typescript", "^5", true),
        ("@types/node", "^20", true),
        ("@types/react", "^19", true),
        ("@types/react-dom", "^19", true),
        ("eslint-config-next", "15.3.2", true),
    ];

    for (name, version, is_dev) in deps_to_ensure.iter() {
        if ensure_dependency_internal(project_dir, &mut package_data, name, version, *is_dev).await? {
            modified_package_json = true;
        }
    }

    let scripts_to_ensure = [
        ("lint", "next lint ./src --format json"),
        ("format", "npx prettier . --write"),
        ("lsp", "typescript-language-server --stdio"),
        ("dev", "next dev --turbopack"),
        ("build", "next build"),
        ("start", "next start"),
    ];

    for (name, command) in scripts_to_ensure.iter() {
        if ensure_script_internal(&mut package_data, name, command) {
            modified_package_json = true;
        }
    }

    if modified_package_json {
        tracing::info!(target: "dev_setup::nextjs", path = %package_json_path.display(), "package.json was modified. Updating and running npm install.");
        let updated_content = serde_json::to_string_pretty(&package_data)
            .context("dev_setup::nextjs: Failed to serialize updated package.json data")?;
        fs::write(&package_json_path, updated_content).with_context(|| {
            format!(
                "dev_setup::nextjs: Failed to write updated package.json to {}",
                package_json_path.display()
            )
        })?;
        terminal::npm::run_npm_command(project_dir, &["install", "--loglevel", "error"], false)
            .await
            .context("dev_setup::nextjs: Final 'npm install' failed after updating package.json. Node modules might be inconsistent.")?;
        tracing::info!(target: "dev_setup::nextjs", "npm install completed after package.json modifications.");
    } else {
        tracing::debug!(target: "dev_setup::nextjs", path = %package_json_path.display(), "package.json was already up-to-date. No modifications needed.");
    }

    Ok(())
}

pub async fn ensure_next_config_rewrites(project_dir: &Path) -> Result<()> {
    let config_filenames = ["next.config.ts", "next.config.js", "next.config.mjs"];
    let mut existing_config_path: Option<PathBuf> = None;
    let mut chosen_config_filename = "next.config.ts"; // Default to .ts for creation

    for filename in &config_filenames {
        let current_path = project_dir.join(filename);
        if current_path.exists() {
            existing_config_path = Some(current_path);
            chosen_config_filename = filename;
            break;
        }
    }

    let expected_config_content = r#"/** @type {import('next').NextConfig} */
const nextConfig = {
  async rewrites() {
    return [
      {
        source: "/galatea/:path*",
        destination: "http://127.0.0.1:3051/:path*",
      },
    ];
  },
};

export default nextConfig;
"#;

    match existing_config_path {
        Some(config_path) => {
            let content = fs::read_to_string(&config_path).with_context(|| {
                format!(
                    "Failed to read existing Next.js config file at {}",
                    config_path.display()
                )
            })?;

            if content.trim() == expected_config_content.trim() {
                tracing::debug!(
                    target: "dev_setup::nextjs",
                    path = %config_path.display(),
                    "Next.js config is already correctly configured for Galatea rewrite rule."
                );
            } else {
                fs::write(&config_path, expected_config_content).with_context(|| {
                    format!(
                        "Failed to overwrite {} with Galatea rewrite rule.",
                        config_path.display()
                    )
                })?;
                tracing::info!(
                    target: "dev_setup::nextjs",
                    path = %config_path.display(),
                    "Updated Next.js config to ensure Galatea rewrite rule."
                );
            }
        }
        None => {
            let new_config_path = project_dir.join(chosen_config_filename); // Uses "next.config.ts" by default
            fs::write(&new_config_path, expected_config_content).with_context(|| {
                format!(
                    "Failed to create {} at {}",
                    chosen_config_filename,
                    new_config_path.display()
                )
            })?;
            tracing::info!(
                target: "dev_setup::nextjs",
                path = %new_config_path.display(),
                action = "created",
                "Next.js config did not exist. Created with Galatea rewrite rule."
            );
        }
    }

    Ok(())
} 