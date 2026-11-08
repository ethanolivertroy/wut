#!/bin/sh
# Rewrite Formula/wut.rb for a new wut release.
#
#   scripts/bump-formula.sh 0.0.3
#
# Downloads each release tarball's published SHA-256, verifies it against a
# fresh download of the tarball itself, then updates the version, URLs, and
# checksums in the formula. Fails loudly rather than writing a bad formula.
set -eu

REPOSITORY="ethanolivertroy/wut"
ASSETS="
wut-aarch64-apple-darwin.tar.gz
wut-aarch64-unknown-linux-musl.tar.gz
wut-x86_64-apple-darwin.tar.gz
wut-x86_64-unknown-linux-musl.tar.gz
"

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
formula="$script_dir/../Formula/wut.rb"

error() {
  printf 'bump: %s\n' "$*" >&2
  exit 1
}

[ $# -eq 1 ] || error "usage: bump-formula.sh VERSION (for example 0.0.3)"
version=${1#v}
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' ||
  error "invalid version: $1"
[ -f "$formula" ] || error "formula not found: $formula"

require() {
  command -v "$1" >/dev/null 2>&1 || error "$1 is required"
}

require curl
require sha256sum
require awk
require sed
require diff

tag="v$version"
release_url="https://github.com/$REPOSITORY/releases/download/$tag"
work=$(mktemp -d)
trap 'rm -rf "$work"' 0
trap 'exit 1' HUP INT TERM

checksums="$work/checksums"
: > "$checksums"

for asset in $ASSETS; do
  printf 'verifying %s...\n' "$asset" >&2
  curl --proto '=https' --tlsv1.2 -fsSL -o "$work/$asset" "$release_url/$asset" ||
    error "$tag has no asset $asset"
  curl --proto '=https' --tlsv1.2 -fsSL -o "$work/$asset.sha256" "$release_url/$asset.sha256" ||
    error "$tag has no checksum for $asset"

  computed=$(sha256sum "$work/$asset" | awk '{ print $1 }')
  published=$(awk '{ print $1 }' "$work/$asset.sha256")
  [ "$computed" = "$published" ] ||
    error "checksum mismatch for $asset (published $published, downloaded $computed)"
  printf '%s %s\n' "$asset" "$computed" >> "$checksums"
done

updated="$work/wut.rb"
awk -v list="$checksums" -v version="$version" '
BEGIN {
  while ((getline line < list) > 0) {
    split(line, field, " ")
    hash[field[1]] = field[2]
  }
}
{
  if ($1 == "version") {
    sub(/"[^"]*"/, "\"" version "\"")
    print
    next
  }
  if ($1 == "url") {
    sub(/\/v[0-9]+\.[0-9]+\.[0-9]+\//, "/v" version "/")
    for (asset in hash) {
      if (index($0, asset) > 0) pending = asset
    }
    print
    next
  }
  if ($1 == "sha256" && pending != "") {
    sub(/"[^"]*"/, "\"" hash[pending] "\"")
    pending = ""
    print
    next
  }
  print
}
' "$formula" > "$updated" ||
  error "failed to rewrite $formula"

if diff -u "$formula" "$updated"; then
  printf 'formula already at %s; nothing to do\n' "$version" >&2
else
  cp "$updated" "$formula"
  printf 'updated %s to %s\n' "$formula" "$version" >&2
fi
