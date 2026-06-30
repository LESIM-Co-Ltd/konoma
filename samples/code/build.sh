#!/usr/bin/env bash
# Sample shell script — syntax highlighting demo.
set -euo pipefail

name="${1:-world}"
count=3

greet() {
  local who="$1"
  echo "Hello, ${who}!"
}

for i in $(seq 1 "$count"); do
  greet "$name #$i"
done

if [[ -d "target" ]]; then
  echo "target exists" >&2
fi
