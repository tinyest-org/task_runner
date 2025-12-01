#!/bin/sh
# Setup git hooks for development

set -e

echo "Configuring git to use .githooks directory..."
git config core.hooksPath .githooks

echo "Git hooks configured successfully."
echo "Pre-commit hook will now run 'cargo fmt --check' before each commit."
