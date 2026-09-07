#!/usr/bin/env bash
#
# Regenerates the documentation site's Mermaid catalog: the PNGs, the text-mode fallbacks and the
# two manifests under `site/src/assets/mermaid-catalog/{en,ja}/`.
#
# Those files are generated, not committed (see `.gitignore`): konoma draws them itself, so a
# checkout has no catalog until this has been run. The Docs workflow
# (`.github/workflows/pages.yml`) runs the same test before `npm run build`; locally, run this once
# before `npm run build` / `npm run dev`, and again after changing a mermaid sample or the
# renderer.
#
# Runs from any directory: the repository root is resolved from this script's own location.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd -- "$script_dir/../.." && pwd)
cd -- "$repo_root"

# The same flags CI uses for tests: default features, which include `git`. The dump is `#[ignore]`d
# because it writes files into the working tree, so it has to be named explicitly.
cargo test -- --ignored site_mermaid_catalog_dump

out=site/src/assets/mermaid-catalog
for lang in en ja; do
  dir=$out/$lang
  if [ ! -f "$dir/manifest.json" ]; then
    echo "gen-catalog: $dir/manifest.json was not written" >&2
    exit 1
  fi
  png=$(find "$dir" -type f -name '*.png' | wc -l | tr -d ' ')
  txt=$(find "$dir" -type f -name '*.txt' | wc -l | tr -d ' ')
  size=$(du -sh "$dir" | cut -f1 | tr -d ' \t')
  printf '%s: %s PNG, %s text, manifest.json (%s)\n' "$lang" "$png" "$txt" "$size"
done
printf 'total: %s under %s/\n' "$(du -sh "$out" | cut -f1 | tr -d ' \t')" "$out"
