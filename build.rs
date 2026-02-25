// build.rs
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=.githooks");
    println!("cargo:rerun-if-changed=static/dag.html");

    // Ensure static/dag.html exists so include_str! compiles.
    // The real file is built from ui/ (cd ui && bun install && bun run build)
    // but is gitignored, so CI/fresh clones need a placeholder.
    ensure_dag_html();

    // Setup git hooks if we're in a git repository
    setup_git_hooks();
}

fn ensure_dag_html() {
    let dag_path = std::path::Path::new("static/dag.html");
    if !dag_path.exists() {
        std::fs::create_dir_all("static").expect("Failed to create static directory");
        std::fs::write(
            dag_path,
            concat!(
                "<!DOCTYPE html><html><body style=\"font-family:sans-serif;padding:2em\">",
                "<p>DAG viewer not built. Run <code>cd ui &amp;&amp; bun install &amp;&amp; bun run build</code></p>",
                "</body></html>",
            ),
        )
        .expect("Failed to write placeholder dag.html");
        println!(
            "cargo:warning=static/dag.html not found, created placeholder. Build UI with: cd ui && bun install && bun run build"
        );
    }
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
