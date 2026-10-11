#!/usr/bin/env bash
# Upload raw block segments of finished windows to a GitHub release, one asset per segment (split into <= 1.9 GB parts,
# GitHub's limit is 2 GiB per asset). Hardlinked duplicates (ext segments linked into the main window) go up once.
# Reassemble: cat <name>.part* > <name>. Needs GITHUB_TOKEN in .env (never printed).
# Usage: scripts/gh_upload_raw.sh TAG WINDOW...
set -euo pipefail
cd "$(dirname "$0")/.."
TAG=$1; shift
REPO=horiii726-debug/sniper_v4
set -a; . ./.env; set +a
API=https://api.github.com/repos/$REPO
AUTH=(-H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/vnd.github+json")
id=$(curl -sf "${AUTH[@]}" "$API/releases/tags/$TAG" | python3 -c "import json,sys; print(json.load(sys.stdin)['id'])")
have() { curl -sf "${AUTH[@]}" "$API/releases/$id/assets?per_page=100&page=$1" | python3 -c "import json,sys; print(' '.join(a['name'] for a in json.load(sys.stdin)))"; }
HAVE=" $(have 1) $(have 2) $(have 3) "
PART=1900000000
TMP=${TMPDIR:-/tmp}/gh_raw_parts; mkdir -p "$TMP"
declare -A seen
for w in "$@"; do
  for f in data/rawkeep/v5_$w/raw-*.zst data/rawkeep/v5_${w}_ext/raw-*.zst; do
    [ -f "$f" ] || continue
    ino=$(stat -c %i "$f")
    [ -n "${seen[$ino]:-}" ] && { echo "skip $f (hardlink of ${seen[$ino]})"; continue; }
    seen[$ino]=$f
    base="$(basename "$(dirname "$f")")__$(basename "$f")"
    size=$(stat -c %s "$f")
    n=$(( (size + PART - 1) / PART ))
    for ((k=0; k<n; k++)); do
      name=$base; [ "$n" -gt 1 ] && name=$(printf '%s.part%02d' "$base" "$k")
      case "$HAVE" in *" $name "*) echo "skip $name (uploaded)"; continue ;; esac
      echo "$(date -u +%T) upload $name"
      src=$f
      if [ "$n" -gt 1 ]; then   # part written to disk first: curl streams files (-T) but buffers stdin in memory
        src=$TMP/$name
        dd if="$f" of="$src" bs=64M iflag=skip_bytes,count_bytes skip=$((k * PART)) count=$PART status=none
      fi
      curl -sf "${AUTH[@]}" -H "Content-Type: application/octet-stream" -X POST -T "$src" \
        "https://uploads.github.com/repos/$REPO/releases/$id/assets?name=$name" > /dev/null
      [ "$src" = "$f" ] || rm -f "$src"
    done
  done
done
echo RAW_UPLOAD_DONE
