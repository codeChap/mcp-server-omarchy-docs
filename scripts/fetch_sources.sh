#!/usr/bin/env bash
# Refresh raw doc sources: the basecamp/omarchy repo, GitHub release notes,
# and omarchyplugins.com authoring guides.
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

# Marketplace plugin authoring guides (not in basecamp/omarchy).
echo "Fetching omarchyplugins.com authoring guides..."
mkdir -p data/raw/omarchyplugins
for page in develop.html publish.html; do
    if curl -fsSL -A "mcp-server-omarchy-docs/kb" \
        "https://omarchyplugins.com/${page}" \
        -o "data/raw/omarchyplugins/${page}"; then
        echo "  ${page}"
    else
        echo "  ! failed to fetch ${page} (build-kb will skip it)" >&2
        rm -f "data/raw/omarchyplugins/${page}"
    fi
done

echo "Done."
