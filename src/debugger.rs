use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing;

use crate::resolver;
use crate::utils;
use crate::logging::{self, LogLevel, LogSource};

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

    let output = child.wait_with_output().await.with_context(|| {
        format!(
            "Debugger: Failed to wait for npm command: npm {}",
            args.join(" ")
        )
    })?;

    if output.status.success() {
        if !suppress_output {
            let stdout_data = String::from_utf8_lossy(&output.stdout);
            if !stdout_data.is_empty() {
                println!(
                    "Debugger: npm stdout:
{}",
                    stdout_data
                );
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
        Some(current_version) => current_version != target_version,
        None => true,
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
            .with_context(|| {
                format!(
                    "Debugger: Failed to install/update dependency '{}'",
                    dep_name
                )
            })?;

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
        Some(current_command) if current_command == script_command => {}
        _ => {
            package_json_data
                .scripts
                .insert(script_name.to_string(), script_command.to_string());
            modified = true;
        }
    }
    modified
}

async fn ensure_next_config_rewrites(project_dir: &Path) -> Result<()> {
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
                    "Debugger: Failed to read existing Next.js config file at {}",
                    config_path.display()
                )
            })?;

            if content.trim() == expected_config_content.trim() {
                println!(
                    "Debugger: {} is already correctly configured for Galatea rewrite rule.",
                    config_path.display()
                );
            } else {
                fs::write(&config_path, expected_config_content).with_context(|| {
                    format!(
                        "Debugger: Failed to overwrite {} with Galatea rewrite rule.",
                        config_path.display()
                    )
                })?;
                println!(
                    "Debugger: Updated {} to ensure Galatea rewrite rule.",
                    config_path.display()
                );
            }
        }
        None => {
            let new_config_path = project_dir.join(chosen_config_filename); // Uses "next.config.ts" by default
            fs::write(&new_config_path, expected_config_content).with_context(|| {
                format!(
                    "Debugger: Failed to create {} at {}",
                    chosen_config_filename,
                    new_config_path.display()
                )
            })?;
            println!(
                "Debugger: Created {} with Galatea rewrite rule as it did not exist.",
                new_config_path.display()
            );
        }
    }

    Ok(())
}

pub async fn verify_and_setup_project() -> Result<()> {
    let project_dir = resolver::get_project_root().context("Debugger: Failed to get project root. Ensure 'project' subdirectory exists next to the executable.")?;

    let package_json_path = project_dir.join("package.json");

    if !package_json_path.exists() {
        println!(
            "Debugger: Initializing new package.json in {}...",
            project_dir.display()
        );
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
    if ensure_dependency(
        &project_dir,
        &mut package_data,
        "react-dom",
        "^19.0.0",
        false,
    )
    .await?
    {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "eslint", "^9", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "prettier", "3.5.3", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(
        &project_dir,
        &mut package_data,
        "typescript-language-server",
        "^4.3.4",
        true,
    )
    .await?
    {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "typescript", "^5", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "@types/node", "^20", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(&project_dir, &mut package_data, "@types/react", "^19", true).await? {
        modified_package_json = true;
    }
    if ensure_dependency(
        &project_dir,
        &mut package_data,
        "@types/react-dom",
        "^19",
        true,
    )
    .await?
    {
        modified_package_json = true;
    }
    if ensure_dependency(
        &project_dir,
        &mut package_data,
        "eslint-config-next",
        "15.3.2",
        true,
    )
    .await?
    {
        modified_package_json = true;
    }

    if ensure_script(&mut package_data, "lint", "next lint ./src --format json") {
        modified_package_json = true;
    }
    if ensure_script(&mut package_data, "format", "npx prettier . --write") {
        modified_package_json = true;
    }
    if ensure_script(
        &mut package_data,
        "lsp",
        "typescript-language-server --stdio",
    ) {
        modified_package_json = true;
    }
    if ensure_script(&mut package_data, "dev", "next dev --turbopack") {
        modified_package_json = true;
    }
    if ensure_script(&mut package_data, "build", "next build") {
        modified_package_json = true;
    }
    if ensure_script(&mut package_data, "start", "next start") {
        modified_package_json = true;
    }

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

    ensure_next_config_rewrites(&project_dir)
        .await
        .context("Debugger: Failed to ensure Next.js config for Galatea rewrites.")?;

    Ok(())
}

pub async fn start_dev_server(project_dir: &Path) -> Result<()> {
    utils::ensure_port_is_free(3000, "Next.js dev server")
        .await
        .context("Failed to ensure Next.js dev server port (3000) is free before starting")?;

    logging::add_log_entry(
        LogSource::DebuggerGeneral,
        LogLevel::Info,
        format!("Attempting to start 'npm run dev' in {}", project_dir.display()),
    );
    tracing::info!(
        target: "galatea::debugger",
        project_dir = %project_dir.display(),
        "Attempting to start 'npm run dev'"
    );

    let mut cmd = Command::new("npm");
    cmd.current_dir(project_dir);
    cmd.args(&["run", "dev"]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().with_context(|| {
        format!(
            "Debugger: Failed to spawn 'npm run dev' in {}. Ensure npm is installed and the script exists.",
            project_dir.display()
        )
    })?;

    let stdout = child
        .stdout
        .take()
        .context("Debugger: Failed to capture stdout from 'npm run dev'")?;
    let stderr = child
        .stderr
        .take()
        .context("Debugger: Failed to capture stderr from 'npm run dev'")?;

    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            logging::add_log_entry(
                LogSource::DebuggerNpmStdout, 
                LogLevel::Info, 
                line.clone()
            );
            tracing::info!(target: "galatea::debugger::npm_dev_stdout", source_process = "next_dev_server", "{}", line);
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            logging::add_log_entry(
                LogSource::DebuggerNpmStderr, 
                LogLevel::Warn,
                line.clone()
            );
            tracing::warn!(target: "galatea::debugger::npm_dev_stderr", source_process = "next_dev_server", "{}", line);
        }
    });

    let status = child
        .wait()
        .await
        .with_context(|| "Debugger: 'npm run dev' process failed to wait")?;

    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if status.success() {
        let success_msg = format!("'npm run dev' completed successfully (status: {}).", status);
        logging::add_log_entry(LogSource::DebuggerGeneral, LogLevel::Info, success_msg.clone());
        tracing::info!(target: "galatea::debugger", source_process = "next_dev_server", "{}", success_msg);
        Ok(())
    } else {
        let err_msg = format!(
            "Debugger: 'npm run dev' exited with status: {}. Check output above for details.",
            status
        );
        logging::add_log_entry(LogSource::DebuggerGeneral, LogLevel::Error, err_msg.clone());
        tracing::error!(target: "galatea::debugger", source_process = "next_dev_server", "{}", err_msg);
        Err(anyhow!(
            "{}",
            err_msg
        ))
    }
}
