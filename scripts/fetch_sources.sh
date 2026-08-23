#!/usr/bin/env bash
# Refresh raw doc sources: the basecamp/omarchy repo and GitHub release notes.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p data/raw

if [ -d data/raw/omarchy-repo/.git ]; then
    echo "Updating basecamp/omarchy..."
    git -C data/raw/omarchy-repo pull --ff-only
else
    # Full history but no file blobs: the KB indexes commit messages,
    # and blobs aren't needed for those.
    echo "Cloning basecamp/omarchy..."
    git clone --depth 1 --filter=blob:none https://github.com/basecamp/omarchy.git data/raw/omarchy-repo
    git -C data/raw/omarchy-repo fetch --deepen=1000000 -q
fi

echo "Fetching release notes..."
curl -fsSL "https://api.github.com/repos/basecamp/omarchy/releases?per_page=100" -o data/raw/releases.json

# Official site search index: maps manual chapter titles to live omarchy.org URLs.
echo "Fetching manual search index..."
curl -fsSL "https://omarchy.org/manual/search-index.json" -o data/raw/manual-search-index.json

echo "Done."
