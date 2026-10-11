#!/usr/bin/env bash
# Push the work branch and upload the processed data bundles as a GitHub release.
# Needs GITHUB_TOKEN (fine-grained, this repo, Contents: read & write) in .env. The token is never printed.
# Usage: scripts/gh_publish.sh [branch] [release_tag]
set -euo pipefail
cd "$(dirname "$0")/.."
BRANCH=${1:-v6-impl}
TAG=${2:-data-v6-$(date -u +%F)}
REPO=horiii726-debug/sniper_v4
set -a; . ./.env; set +a
[ -n "${GITHUB_TOKEN:-}" ] || { echo "GITHUB_TOKEN missing in .env"; exit 1; }
# shellcheck disable=SC2016
git -c credential.helper='!f() { echo username=x-access-token; echo "password=$GITHUB_TOKEN"; }; f' push -u origin "$BRANCH"
API=https://api.github.com/repos/$REPO
AUTH=(-H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/vnd.github+json")
rel=$(curl -sf "${AUTH[@]}" "$API/releases/tags/$TAG" || true)
if [ -z "$rel" ]; then
  rel=$(curl -sf "${AUTH[@]}" -X POST "$API/releases" \
    -d "{\"tag_name\":\"$TAG\",\"target_commitish\":\"$BRANCH\",\"name\":\"$TAG\",\"body\":\"Processed V6 windows (decoded events, trades, 1 h snapshots, metadata). See docs/V6_STATUS.md. Raw segments list: RAW_SEGMENTS.txt.\",\"prerelease\":true}")
fi
id=$(printf '%s' "$rel" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
have=$(curl -sf "${AUTH[@]}" "$API/releases/$id/assets?per_page=100" | python3 -c "import json,sys; print(' '.join(a['name'] for a in json.load(sys.stdin)))")
for f in data/export/*; do
  n=$(basename "$f")
  case " $have " in *" $n "*) echo "skip $n (already uploaded)"; continue ;; esac
  echo "upload $n ($(du -h "$f" | cut -f1))"
  curl -sf "${AUTH[@]}" -H "Content-Type: application/octet-stream" --data-binary @"$f" \
    "https://uploads.github.com/repos/$REPO/releases/$id/assets?name=$n" > /dev/null
done
echo "done: https://github.com/$REPO/releases/tag/$TAG"
