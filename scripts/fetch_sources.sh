#!/usr/bin/env bash
# Refresh raw doc sources: the basecamp/omarchy repo and GitHub release notes.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p data/raw

if [ -d data/raw/omarchy-repo/.git ]; then
    echo "Updating basecamp/omarchy..."
    git -C data/raw/omarchy-repo pull --ff-only
else
    echo "Cloning basecamp/omarchy..."
    git clone --depth 1 https://github.com/basecamp/omarchy.git data/raw/omarchy-repo
fi

echo "Fetching release notes..."
curl -fsSL "https://api.github.com/repos/basecamp/omarchy/releases?per_page=100" -o data/raw/releases.json

echo "Done."
