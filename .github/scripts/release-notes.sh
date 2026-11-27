#!/usr/bin/env bash
# The GitHub Release notes for a tag, from its CHANGELOG.md entry. release.yml runs it on every
# v* tag, and it serves for older tags by hand: `.github/scripts/release-notes.sh v0.4.0`.
# Relative links become links into the tagged tree, so they work on the Releases page.
# A major release (X.0.0) needs its upgrade page, docs/whats-new-X.0.md, and the notes link it:
# a major without that page fails here, before anything is published.
set -euo pipefail
tag="${1:?usage: release-notes.sh vX.Y.Z}"
version="${tag#v}"
repo="${GITHUB_REPOSITORY:-GeiserX/OwnHealth}"
cd "$(dirname "$0")/../.."

notes=$(awk -v h="## $version " 'index($0, h) == 1 { f = 1; next } f && /^## / { exit } f' CHANGELOG.md | sed '/./,$!d')
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
  echo "release-notes: CHANGELOG.md has no '## $version ' entry" >&2
  exit 1
fi

case "$version" in
  *.0.0)
    major="${version%%.*}"
    page="docs/whats-new-$major.0.md"
    if [ ! -f "$page" ]; then
      echo "release-notes: $version is a major release and $page is missing" >&2
      exit 1
    fi
    notes="Upgrading? Read **What's new in $major.0**, with every upgrade step: https://geiserx.github.io/OwnHealth/whats-new-$major.0/

$notes"
    ;;
esac

printf '%s\n' "$notes" | TAG="$tag" REPO="$repo" perl -pe \
  's{\]\((?!https?://|#|mailto:)([^)\s]+)\)}{](https://github.com/$ENV{REPO}/blob/$ENV{TAG}/$1)}g'
