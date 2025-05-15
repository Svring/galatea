use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

use crate::resolver;

// --- Transplanted from watcher.rs ---
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

async fn run_npm_command(project_dir: &Path, args: &[&str], suppress_output: bool) -> Result<()> {
    let mut cmd = Command::new("npm");
    cmd.current_dir(project_dir);
    cmd.args(args);

    match suppress_output {
        true => {
            cmd.stdout(Stdio::null());
            cmd.stderr(Stdio::null());
        }
        false => {
            cmd.stdout(Stdio::piped());
            cmd.stderr(Stdio::piped());
        }
    }

    let child = cmd.spawn().with_context(|| {
        format!(
            "Debugger: Failed to spawn npm command (npm {}). Ensure npm is installed and in PATH.",
            args.join(" ")
        )
    })?;

    let output = child
        .wait_with_output()
        .await
        .with_context(|| format!("Debugger: Failed to wait for npm command: npm {}", args.join(" ")))?;

    if output.status.success() {
        if !suppress_output {
            let stdout_data = String::from_utf8_lossy(&output.stdout);
            if !stdout_data.is_empty() {
                println!("Debugger: npm stdout:
{}", stdout_data);
            }
        }
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(anyhow!(
            "Debugger: npm command failed with status: {}.
Command: npm {}
Stderr: {}
Stdout: {}",
            output.status,
            args.join(" "),
            stderr,
            stdout
        ))
    }
}
// --- End of Transplanted code ---

async fn ensure_dependency(
    project_dir: &Path,
    package_json_data: &mut PackageJsonData,
    dep_name: &str,
    target_version: &str, // e.g., "15.3.2" or "^9.0.0"
    is_dev_dependency: bool,
) -> Result<bool> {
    let mut modified = false;
    let dep_map = if is_dev_dependency {
        &mut package_json_data.dev_dependencies
    } else {
        &mut package_json_data.dependencies
    };

    let needs_install_or_update = match dep_map.get(dep_name) {
        Some(current_version) => {
            current_version != target_version
        }
        None => {
            true
        }
    };

    if needs_install_or_update {
        let mut install_args = vec!["install", "--loglevel", "error"];
        if is_dev_dependency {
            install_args.push("--save-dev");
        }
        let dep_with_version = format!("{}@{}", dep_name, target_version);
        install_args.push(&dep_with_version);

        run_npm_command(project_dir, &install_args, false)
            .await
            .with_context(|| format!("Debugger: Failed to install/update dependency '{}'", dep_name))?;
        
        dep_map.insert(dep_name.to_string(), target_version.to_string());
        modified = true;
    }
    Ok(modified)
}

fn ensure_script(
    package_json_data: &mut PackageJsonData,
    script_name: &str,
    script_command: &str,
) -> bool {
    let mut modified = false;
    match package_json_data.scripts.get(script_name) {
        Some(current_command) if current_command == script_command => {
        }
        _ => {
            package_json_data
                .scripts
                .insert(script_name.to_string(), script_command.to_string());
            modified = true;
        }
    }
    modified
}

pub async fn verify_and_setup_project() -> Result<()> {
    let project_dir = resolver::get_project_root().context("Debugger: Failed to get project root. Ensure 'project' subdirectory exists next to the executable.")?;
    
    let package_json_path = project_dir.join("package.json");

    if !package_json_path.exists() {
        println!("Debugger: Initializing new package.json in {}...", project_dir.display());
        run_npm_command(&project_dir, &["init", "-y"], false)
            .await
            .context("Debugger: Failed to initialize new package.json. Please create it manually or check npm installation.")?;
        println!("Debugger: package.json initialized. Please review and customize if necessary.");
    }
    
    let content = fs::read_to_string(&package_json_path).with_context(|| {
        format!(
            "Debugger: Failed to read package.json from {}",
            package_json_path.display()
        )
    })?;

    let mut package_data: PackageJsonData = serde_json::from_str(&content).with_context(|| {
        format!(
            "Debugger: Failed to parse package.json from {}. Content: {}",
            package_json_path.display(),
            content
        )
    })?;

    let mut modified_package_json = false;

    if ensure_dependency(&project_dir, &mut package_data, "next", "15.3.2", false).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "react", "^19.0.0", false).await? {
         modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "react-dom", "^19.0.0", false).await? {
         modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "eslint", "^9", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "prettier", "3.5.3", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "typescript-language-server", "^4.3.4", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "typescript", "^5", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "@types/node", "^20", true).await? { modified_package_json = true; }
    if ensure_dependency(&project_dir, &mut package_data, "@types/react", "^19", true).await? { modified_package_json = true; }
    if ensure_dependency(&project_dir, &mut package_data, "@types/react-dom", "^19", true).await? { modified_package_json = true; }
    if ensure_dependency(&project_dir, &mut package_data, "eslint-config-next", "15.3.2", true).await? { modified_package_json = true; }

    if ensure_script(
        &mut package_data,
        "lint",
        "next lint ./src --format json",
    ) {
        modified_package_json = true;
    }
    if ensure_script(
        &mut package_data,
        "format",
        "npx prettier . --write",
    ) {
        modified_package_json = true;
    }
    if ensure_script(
        &mut package_data,
        "lsp",
        "typescript-language-server --stdio",
    ) {
        modified_package_json = true;
    }
    if ensure_script(&mut package_data, "dev", "next dev --turbopack") { modified_package_json = true; }
    if ensure_script(&mut package_data, "build", "next build") { modified_package_json = true; }
    if ensure_script(&mut package_data, "start", "next start") { modified_package_json = true; }

    if modified_package_json {
        let updated_content = serde_json::to_string_pretty(&package_data)
            .context("Debugger: Failed to serialize updated package.json data")?;
        fs::write(&package_json_path, updated_content).with_context(|| {
            format!(
                "Debugger: Failed to write updated package.json to {}",
                package_json_path.display()
            )
        })?;

        run_npm_command(&project_dir, &["install", "--loglevel", "error"], false)
            .await
            .context("Debugger: Final 'npm install' failed after updating package.json. Node modules might be inconsistent.")?;

    }

    Ok(())
}

// Example of how this might be called (e.g., from a CLI command in main.rs)
// pub async fn run_debugger_cli() {
//     if let Err(e) = verify_and_setup_project().await {
//         eprintln!("Project debugger failed: {:?}", e);
//     }
// }
