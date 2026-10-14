#!/usr/bin/env bash
# Refresh raw doc sources: the omacom/omarchy repo, GitHub release notes,
# and omarchyplugins.com authoring guides.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p data/raw

REPO_URL="https://github.com/omacom/omarchy.git"
RELEASES_URL="https://api.github.com/repos/omacom/omarchy/releases?per_page=100"

if [ -d data/raw/omarchy-repo/.git ]; then
    echo "Updating omacom/omarchy..."
    git -C data/raw/omarchy-repo remote set-url origin "$REPO_URL"
    git -C data/raw/omarchy-repo pull --ff-only
else
    # Full history but no file blobs: the KB indexes commit messages,
    # and blobs aren't needed for those.
    echo "Cloning omacom/omarchy..."
    git clone --depth 1 --filter=blob:none "$REPO_URL" data/raw/omarchy-repo
    git -C data/raw/omarchy-repo fetch --deepen=1000000 -q
fi

echo "Fetching release notes..."
curl -fsSL "$RELEASES_URL" -o data/raw/releases.json

# Maps chapter titles to live omarchy.org URLs for build-kb.
# The site used to publish /manual/search-index.json; the Astro rewrite
# dropped that file. Prefer it when present, otherwise derive URLs from
# local manual/*.md (NN-slug.md -> /manual/slug/, welcome -> /manual/).
echo "Fetching manual search index..."
if curl -fsSL "https://omarchy.org/manual/search-index.json" -o data/raw/manual-search-index.json; then
    echo "  from omarchy.org/manual/search-index.json"
else
    echo "  live search-index.json missing; generating from local manual/"
    python3 - <<'PY'
import json, re
from pathlib import Path

manual = Path("data/raw/omarchy-repo/manual")
entries = []
for path in sorted(manual.glob("*.md")):
    stem = path.stem
    slug = re.sub(r"^\d+-", "", stem)
    text = path.read_text(encoding="utf-8")
    match = re.search(r"^#\s+(.+)$", text, re.M)
    chapter = match.group(1).strip() if match else slug.replace("-", " ").title()
    url = "/manual/" if slug == "welcome-to-omarchy" else f"/manual/{slug}/"
    entries.append({"chapter": chapter, "title": chapter, "url": url, "text": ""})
out = Path("data/raw/manual-search-index.json")
out.write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")
print(f"  wrote {len(entries)} chapter URLs")
PY
fi

# Marketplace plugin authoring guides (not in omacom/omarchy).
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
