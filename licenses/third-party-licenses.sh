#!/bin/sh
# Regenerates THIRD-PARTY-LICENSES.md, the license list shipped in the release archives and in
# the crates.io package. Needs cargo-about and python3.
#
#   cargo install cargo-about --features cli    # once
#   sh licenses/third-party-licenses.sh
#
# It lists the dependency versions of Cargo.lock for the three released targets. Run it after
# changing dependencies and commit the result. The release workflow neither regenerates nor
# checks it.
#
# Maintained by hand, because neither cargo-about nor embedded_notices.py can derive them:
# licenses/katex.md, licenses/gemoji.md and the end of about.hbs. embedded_notices.py also names
# every file it relies on, but it cannot notice material that a new dependency (or a new version
# of one) embeds: look for that when dependencies change.
#
# The list is assembled in a temporary file and moved into place only if every step succeeded.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd -P)

crates=$(mktemp)
out=$(mktemp "$root/.third-party-licenses.XXXXXX")
trap 'rm -f "$crates" "$out"' EXIT

cargo about generate --fail --locked --manifest-path "$root/Cargo.toml" \
    -c "$root/about.toml" -o "$crates" "$root/about.hbs"

# A separate step each, so that a failure stops the script (a pipeline would hide it).
{
    cat "$crates"
    printf '\n'
    python3 -I "$root/licenses/embedded_notices.py" "$root/Cargo.toml"
    cat "$root/licenses/katex.md"
    printf '\n'
    cat "$root/licenses/gemoji.md"
} >"$out"

chmod 644 "$out"
mv "$out" "$root/THIRD-PARTY-LICENSES.md"
