---
type: process
status: proposed
---

# Git Etiquette

## Motivation

In a large project with multiple contributors, it is necessary to establish some git etiquette to keep the project maintainable. The following are the guiding principles:

- make git history more readable
- allow automatic changelog generation
- promote self-contained PRs that are easier to review
- promote continuous integration

## Decision

Contributors will follow these git practices to keep the project maintainable.

- use trunk-based development
- maintain a linear history on trunk (`main` branch)
- apply changes via PRs
- use squash merge for all PRs
- use conventional commits for git messages
