mod batch;
mod dag;
mod query;
mod task;

// Re-export all DTOs for backward compatibility
pub use batch::*;
pub use dag::*;
pub use query::*;
pub use task::*;

/// Escape LIKE wildcards in user input to prevent pattern injection.
pub(crate) fn escape_like_pattern(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
