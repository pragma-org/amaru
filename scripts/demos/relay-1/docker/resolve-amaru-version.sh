#!/usr/bin/env bash

# Prints a concrete amaru release version, resolving `latest` through the GitHub API and stripping a
# leading v from anything else.
#
# The multi-architecture workflow resolves the version once with this script and hands the result to
# both builds. Resolving it inside each build instead would let a release published mid-run put two
# different amaru versions in the same manifest list.

set -euo pipefail

version="${1:?usage: resolve-amaru-version.sh <version|latest>}"

if [[ "$version" != latest ]]; then
  echo "${version#v}"
  exit 0
fi

# Every amaru release is marked pre-release, so the /releases/latest endpoint returns 404; take the
# most recent entry of the release list instead.
tag="$(curl -fsSL 'https://api.github.com/repos/pragma-org/amaru/releases?per_page=1' | jq -r '.[0].tag_name // empty')"
[[ -n "$tag" ]] || { echo "error: could not resolve the latest amaru release from the GitHub API" >&2; exit 1; }
echo "${tag#v}"
