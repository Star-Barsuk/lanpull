#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Publish the already-built release to GitHub with `gh`.
#
# The archive, SHA256SUMS, and COMMIT must exist in dist/ from `make release`.
# The script verifies the checksum and requires the annotated tag to exist, to
# point at HEAD, and to carry the same hash. It never creates or pushes a tag;
# create and push it as described in docs/RELEASE.md. It can create a draft for
# review or replace the assets of an existing release.
#
# Usage: publish.sh [--draft] [--clobber]

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
cd "$root"

# gh is commonly installed user-locally; do not require a PATH edit.
PATH="$HOME/.local/bin:$PATH"
export PATH

draft=0
clobber=0
for arg in "$@"; do
    case "$arg" in
        --draft) draft=1 ;;
        --clobber) clobber=1 ;;
        *) printf 'publish: unknown argument: %s\n' "$arg" >&2; exit 1 ;;
    esac
done

die() {
    printf 'publish: %s\n' "$*" >&2
    exit 1
}

command -v gh >/dev/null 2>&1 || die "gh is missing; install the GitHub CLI"
command -v sha256sum >/dev/null 2>&1 || die "sha256sum is required"
gh auth status >/dev/null 2>&1 || die "gh is not authenticated; run 'gh auth login' once"

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' server/Cargo.toml | head -n 1)
[ -n "$version" ] || die "cannot read the workspace version"
tag="v$version"
tar="$root/dist/lanpull-$tag.tar.gz"
sums="$root/dist/SHA256SUMS"
notes_dir="$root/dist/lanpull-$tag"

[ -f "$tar" ] || die "missing $tar; run 'make release' first"
[ -f "$sums" ] || die "missing $sums; run 'make release' first"
[ -f "$notes_dir/RELEASE_NOTES.md" ] || die "missing $notes_dir/RELEASE_NOTES.md; run 'make release' first"
[ -f "$root/dist/COMMIT" ] || die "missing dist/COMMIT; re-run 'make release' from the tagged commit"
(cd "$root/dist" && sha256sum -c SHA256SUMS >/dev/null) || die "dist/SHA256SUMS does not match the archive"
hash=$(cd "$root/dist" && sha256sum "lanpull-$tag.tar.gz" | cut -d' ' -f1)

dist_commit=$(tr -d '[:space:]' <"$root/dist/COMMIT")
head_commit=$(git rev-parse HEAD)
[ "$dist_commit" = "$head_commit" ] \
    || die "dist was built from $dist_commit, HEAD is $head_commit; rebuild the archive"

git rev-parse -q --verify "refs/tags/$tag" >/dev/null \
    || die "tag $tag does not exist; create and push it first (see docs/RELEASE.md)"
annotated=$(git tag -l --format='%(contents)' "$tag")
printf '%s\n' "$annotated" | grep -F "$hash" >/dev/null \
    || die "tag $tag does not carry the archive hash; refusing to publish"
tag_commit=$(git rev-parse "$tag^{commit}")
[ "$tag_commit" = "$head_commit" ] \
    || die "tag $tag points to $tag_commit, HEAD is $head_commit; publish the tagged commit"

notes=$(mktemp)
trap 'rm -f "$notes"' EXIT
cat "$notes_dir/RELEASE_NOTES.md" >"$notes"
{
    printf '\n---\n\n'
    # shellcheck disable=SC2016 # backticks are Markdown, not command substitution
    printf 'SHA256 (`lanpull-%s.tar.gz`): `%s`\n\n' "$tag" "$hash"
    # shellcheck disable=SC2016 # backticks are Markdown, not command substitution
    printf 'Verify with `sha256sum -c SHA256SUMS` and compare the value with the tag message.\n'
} >>"$notes"

if gh release view "$tag" >/dev/null 2>&1; then
    if [ "$clobber" = 1 ]; then
        gh release upload "$tag" "$tar" "$sums" --clobber
        gh release edit "$tag" --notes-file "$notes"
        printf 'publish: replaced assets and notes for %s\n' "$tag"
    else
        die "release $tag already exists; pass --clobber to replace its assets"
    fi
elif [ "$draft" = 1 ]; then
    gh release create "$tag" --verify-tag --draft --title "lanpull $version" \
        --notes-file "$notes" "$tar" "$sums"
else
    gh release create "$tag" --verify-tag --title "lanpull $version" \
        --notes-file "$notes" "$tar" "$sums"
fi

url=$(gh release view "$tag" --json url --jq .url)
printf 'publish: %s\n' "$url"
gh release view "$tag" --json assets --jq '.assets[] | "  \(.name)  \(.size) bytes"'
if [ "$draft" = 1 ]; then
    printf 'publish: draft; review it, then run: gh release edit %s --draft=false\n' "$tag"
fi
