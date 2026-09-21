#!/usr/bin/env bash
set -euo pipefail
# Delete only our reproducible caches. Never change the repository's paid cap.
budget=$((8 * 1024 * 1024 * 1024))
inventory="${RUNNER_TEMP:?}/mountmate-cache-inventory.json"
gh api --paginate --slurp "repos/${GITHUB_REPOSITORY:?}/actions/caches?per_page=100" \
  | jq '[.[].actions_caches[]]' > "$inventory"
total=$(jq '[.[].size_in_bytes] | add // 0' "$inventory")
before=$total
deleted=0
# Prefer keeping expensive Windows/Intel macOS builds and compact rclone binaries.
# Within each tier, evict the least recently used entry first.
while IFS=$'\t' read -r id size; do
  if (( total <= budget )); then break; fi
  gh api --method DELETE "repos/${GITHUB_REPOSITORY}/actions/caches/${id}"
  total=$((total - size))
  deleted=$((deleted + 1))
done < <(jq -r '
  [.[] | select(.key | startswith("mountmate-")) |
    . + {priority: (if (.key | startswith("mountmate-rclone-")) then 2
      elif (.key | test("windows|macos-15-intel")) then 1 else 0 end)}]
  | sort_by(.priority, .last_accessed_at)[] | [.id, .size_in_bytes] | @tsv
' "$inventory")
{
  echo "Cache storage: ${before} → ${total} bytes; removed ${deleted} entries; target ${budget} bytes."
  echo "Only mountmate-* caches are eligible; the repository storage cap is unchanged."
} | tee -a "$GITHUB_STEP_SUMMARY"
