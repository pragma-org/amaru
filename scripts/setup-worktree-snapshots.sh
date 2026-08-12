#!/usr/bin/env bash
# Link this worktree's snapshots/ directory to the primary (main) worktree's
# snapshots/, so bootstrap/dev data is shared instead of re-downloaded per worktree.
#
# Safe to run repeatedly. No-ops on the primary worktree, when the main
# snapshots/ directory is missing, or when a real (non-symlink) snapshots/
# path already exists here.
#
# Invoked from the post-checkout hook (git worktree add / branch checkout)
# and can be run manually after creating a worktree:
#   ./scripts/setup-worktree-snapshots.sh

set -euo pipefail

if ! git rev-parse --git-dir >/dev/null 2>&1; then
  echo "setup-worktree-snapshots: not inside a git repository" >&2
  exit 1
fi

repo_root="$(git rev-parse --show-toplevel)"
# First entry of `git worktree list` is always the primary worktree.
main_root="$(git worktree list --porcelain | awk '/^worktree / { print $2; exit }')"

if [[ -z "${main_root}" ]]; then
  echo "setup-worktree-snapshots: could not determine primary worktree" >&2
  exit 1
fi

if [[ "${repo_root}" == "${main_root}" ]]; then
  exit 0
fi

src="${main_root}/snapshots"
dst="${repo_root}/snapshots"

if [[ ! -d "${src}" ]]; then
  # Nothing to share yet; leave the worktree alone.
  exit 0
fi

if [[ -L "${dst}" ]]; then
  current="$(readlink "${dst}")"
  if [[ "${current}" == "${src}" ]]; then
    exit 0
  fi
  echo "setup-worktree-snapshots: replacing snapshots symlink (${current} -> ${src})"
  rm "${dst}"
elif [[ -e "${dst}" ]]; then
  # Real directory or file — do not clobber local data.
  echo "setup-worktree-snapshots: ${dst} already exists and is not a symlink; leaving it alone" >&2
  exit 0
fi

ln -s "${src}" "${dst}"
echo "setup-worktree-snapshots: linked ${dst} -> ${src}"
