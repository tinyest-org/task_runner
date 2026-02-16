// build.rs
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=.githooks");
    println!("cargo:rerun-if-changed=static/dag.html");

    // Setup git hooks if we're in a git repository
    setup_git_hooks();
}

fn setup_git_hooks() {
    // Only run in the source directory (not when built as a dependency)
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let git_dir = std::path::Path::new(&manifest_dir).join(".git");

    if !git_dir.exists() {
        return;
    }

    // Check if .githooks directory exists
    let hooks_dir = std::path::Path::new(&manifest_dir).join(".githooks");
    if !hooks_dir.exists() {
        return;
    }

    // Configure git to use .githooks directory
    let output = Command::new("git")
        .args(["config", "core.hooksPath", ".githooks"])
        .current_dir(&manifest_dir)
        .output();

    match output {
        Ok(result) if result.status.success() => {
            println!("cargo:warning=Git hooks configured to use .githooks directory");
        }
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            println!("cargo:warning=Failed to configure git hooks: {}", stderr);
        }
        Err(e) => {
            println!("cargo:warning=Could not run git config: {}", e);
        }
    }
}
