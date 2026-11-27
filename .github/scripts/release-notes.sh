#!/usr/bin/env bash
# The GitHub Release notes for a tag, from its CHANGELOG.md entry. release.yml runs it on every
# v* tag, and it serves for older tags by hand: `.github/scripts/release-notes.sh v0.4.0`.
# Relative links become links into the tagged tree, so they work on the Releases page.
# RELEASE_NOTES_REF links another tree instead (main, for a tag cut before a file it links).
# A major release (X.0.0) needs its section in UPGRADING.md, "## Upgrading to X.0", and the notes
# link it: a major without that section fails here, before anything is published.
set -euo pipefail
tag="${1:?usage: release-notes.sh vX.Y.Z}"
version="${tag#v}"
repo="${GITHUB_REPOSITORY:-GeiserX/OwnHealth}"
ref="${RELEASE_NOTES_REF:-$tag}"
cd "$(dirname "$0")/../.."

notes=$(awk -v h="## $version " 'index($0, h) == 1 { f = 1; next } f && /^## / { exit } f' CHANGELOG.md | sed '/./,$!d')
if [ -z "$(printf '%s' "$notes" | tr -d '[:space:]')" ]; then
  echo "release-notes: CHANGELOG.md has no '## $version ' entry" >&2
  exit 1
fi

case "$version" in
  *.0.0)
    major="${version%%.*}"
    if ! grep -qx "## Upgrading to $major.0" UPGRADING.md; then
      echo "release-notes: $version is a major release and UPGRADING.md has no '## Upgrading to $major.0' section" >&2
      exit 1
    fi
    notes="Upgrading? Read [Upgrading to $major.0](UPGRADING.md#upgrading-to-${major}0): what changed and every step.

$notes"
    ;;
esac

printf '%s\n' "$notes" | REF="$ref" REPO="$repo" perl -pe \
  's{\]\((?!https?://|#|mailto:)([^)\s]+)\)}{](https://github.com/$ENV{REPO}/blob/$ENV{REF}/$1)}g'
