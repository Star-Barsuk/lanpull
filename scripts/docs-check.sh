#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Documentation checks that a machine can enforce:
#
# 1. the public tree never references a private document;
# 2. relative Markdown links resolve from the document's own directory;
# 3. private documents and tools stay out of the git index;
# 4. client/VERSION and pull.py __version__ agree.
#
# Prose accuracy is a review concern; this script only catches facts that a
# script can prove wrong.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)
fail=0

note() {
    printf 'docs-check: %s\n' "$*" >&2
    fail=1
}

# --- 1. Public/private boundary -----------------------------------------
# The public tree is every tracked file except private documents and tooling.
# Deriving it from the index means a new public file is scanned automatically.
public_files=$(cd "$root" && {
    git ls-files | grep -Ev '^(AGENTS\.md|tools/|docs/)|(^|/)\.|/fixtures/|^server/fuzz/(corpus|artifacts)/'
    git ls-files docs/CONTRIBUTING.md docs/REFERENCE.md docs/RELEASE.md docs/SECURITY.md
})
allowed_docs="REFERENCE.md SECURITY.md CONTRIBUTING.md RELEASE.md"

# shellcheck disable=SC2086 # the file list is intentionally split into arguments
references=$(cd "$root" && grep -rn -I \
    -E 'docs/[A-Za-z0-9_-]+\.md' $public_files 2>/dev/null || true)
if [ -n "$references" ]; then
    while IFS= read -r line; do
        [ -n "$line" ] || continue
        name=$(printf '%s\n' "$line" | grep -oE 'docs/[A-Za-z0-9_-]+\.md' | head -n 1 | sed 's|^docs/||')
        case " $allowed_docs " in
            *" $name "*) ;;
            *) note "private document referenced from a public file: $line" ;;
        esac
    done <<EOF
$references
EOF
fi

# Public files may only name the public documents. A bare name (without the
# docs/ prefix) is still a reference, so the allowlist is applied to every
# Markdown file name a public file mentions.
allowed_names="README.md CHANGELOG.md REFERENCE.md SECURITY.md CONTRIBUTING.md RELEASE.md THIRD_PARTY_LICENSES.md RELEASE_NOTES.md"
# shellcheck disable=SC2086 # the file list is intentionally split into arguments
mentions=$(cd "$root" && grep -rhoE -I \
    '[A-Za-z0-9_-]+\.md' $public_files 2>/dev/null | sort -u || true)
if [ -n "$mentions" ]; then
    while IFS= read -r name; do
        [ -n "$name" ] || continue
        case " $allowed_names " in
            *" $name "*) ;;
            *) note "public file names a non-public document: $name" ;;
        esac
    done <<EOF
$mentions
EOF
fi

# --- 2. Relative Markdown links -----------------------------------------
documents=$(cd "$root" && find . -name '*.md' \
    -not -path './.git/*' -not -path './.venv/*' -not -path './client/.venv/*' \
    -not -path './server/target/*' -not -path './dist/*' -not -path './tools/*' \
    | sed 's|^\./||' | sort)
while IFS= read -r document; do
    [ -n "$document" ] || continue
    directory=$(dirname "$document")
    links=$(grep -oE '\]\([^)]+' "$root/$document" 2>/dev/null | sed 's/^](//' || true)
    while IFS= read -r link; do
        [ -n "$link" ] || continue
        case "$link" in
            http://*|https://*|mailto:*|'#'*) continue ;;
        esac
        # Drop an optional Markdown title: `path "title"`.
        link=${link%%[[:space:]]*}
        target=${link%%#*}
        [ -n "$target" ] || continue
        case "$target" in
            /*) candidate=$root$target ;;
            *) candidate=$root/$directory/$target ;;
        esac
        if [ ! -e "$candidate" ]; then
            note "$document: broken link: $link"
        fi
    done <<EOF
$links
EOF
done <<EOF
$documents
EOF

# --- 3. Private files stay out of the index -------------------------------
expected_docs='docs/CONTRIBUTING.md
docs/REFERENCE.md
docs/RELEASE.md
docs/SECURITY.md'
tracked_docs=$(cd "$root" && git ls-files docs | sort)
if [ "$tracked_docs" != "$expected_docs" ]; then
    note "tracked docs/ files do not match the public set"
fi
agents_name=$(printf '%s.%s' AGENTS md)
if [ -n "$(cd "$root" && git ls-files "$agents_name")" ]; then
    note "$agents_name is tracked; it is private"
fi
if [ -n "$(cd "$root" && git ls-files tools)" ]; then
    note "tools/ is tracked; it is private"
fi

# --- 4. Client version ---------------------------------------------------
version_file=$root/client/VERSION
if [ ! -f "$version_file" ]; then
    note "client/VERSION is missing"
else
    version=$(tr -d '[:space:]' <"$version_file")
    if ! grep -q "^__version__ = \"$version\"$" "$root/client/pull.py"; then
        note "client/VERSION ($version) and pull.py __version__ disagree"
    fi
fi

if [ "$fail" -eq 0 ]; then
    printf 'docs-check: clean\n'
fi
exit "$fail"
