#!/bin/bash

# Script to set up git hooks for the repository
# Run this after cloning the repository

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOKS_DIR="$SCRIPT_DIR/hooks"
GIT_HOOKS_DIR="$(git rev-parse --git-dir)/hooks"

echo "Setting up git hooks..."

if ! git rev-parse --git-dir > /dev/null 2>&1; then
    echo "Error: Not in a git repository"
    exit 1
fi

if [ ! -d "$HOOKS_DIR" ]; then
    echo "Error: hooks directory not found at $HOOKS_DIR"
    echo "Make sure you're running this from the repository root"
    exit 1
fi

for hook in "$HOOKS_DIR"/*; do
    if [ -f "$hook" ]; then
        hook_name=$(basename "$hook")
        echo "Installing $hook_name hook..."
        cp "$hook" "$GIT_HOOKS_DIR/$hook_name"
        chmod +x "$GIT_HOOKS_DIR/$hook_name"
        echo "$hook_name hook installed"
    fi
done

echo ""
echo "Git hooks setup complete!"
echo ""
echo "The following hooks are now active:"
for hook in "$GIT_HOOKS_DIR"/*; do
    if [ -x "$hook" ] && [ -f "$hook" ]; then
        hook_name=$(basename "$hook")
        if [[ ! "$hook_name" =~ \.sample$ ]]; then
            echo "  - $hook_name"
        fi
    fi
done
