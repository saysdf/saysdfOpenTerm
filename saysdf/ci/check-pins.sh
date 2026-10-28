#!/usr/bin/env bash
# saysdfTerminal: fail if any workflow uses an action not pinned to a 40-char commit SHA,
# or uses a `schedule:` trigger. Local actions (`./...`) and `docker://` are allowed.
# Usage: bash saysdf/ci/check-pins.sh [workflow files...]  (default: saysdf/ci/github-workflows/*.yml|*.yaml)
set -u
[ "$#" -eq 0 ] && { cd "$(dirname "$0")/../.." || exit 2; }
if [ "$#" -gt 0 ]; then files=("$@"); else
  shopt -s nullglob; files=(saysdf/ci/github-workflows/*.yml saysdf/ci/github-workflows/*.yaml); fi
[ "${#files[@]}" -eq 0 ] && { echo "check-pins: no workflow files"; exit 0; }
bad=0
for f in "${files[@]}"; do
  n=0
  while IFS= read -r line; do
    n=$((n+1))
    l="${line%%#*}"   # drop comments (version comments after the SHA are fine)
    if [[ "$l" =~ ^[[:space:]-]*uses:[[:space:]]*[\"\']?([^\"\'[:space:]]+) ]]; then
      ref="${BASH_REMATCH[1]}"
      case "$ref" in ./*|docker://*) continue ;; esac
      if ! [[ "$ref" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}$ ]]; then
        echo "$f:$n: not pinned to a 40-char SHA: $ref"; bad=1
      fi
    fi
    if [[ "$l" =~ ^[[:space:]]*schedule[[:space:]]*: ]]; then
      echo "$f:$n: schedule trigger is forbidden"; bad=1
    fi
  done < <(tr -d '\r' < "$f")
done
[ "$bad" -eq 0 ] && echo "check-pins: ok (${#files[@]} file(s))"
exit "$bad"
