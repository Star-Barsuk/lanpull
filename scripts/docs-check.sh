#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Documentation checks that a machine can enforce:
#
# 1. the public tree never references a private document;
# 2. relative Markdown links resolve;
# 3. client/VERSION and pull.py __version__ agree.
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
# The public files and trees a reader can reach from the repository.
public_paths="README.md docs/REFERENCE.md docs/SECURITY.md docs/CONTRIBUTING.md docs/ENGINEERING_STANDARD.md client server config systemd Makefile scripts"
allowed_docs="REFERENCE.md SECURITY.md CONTRIBUTING.md ENGINEERING_STANDARD.md"

# shellcheck disable=SC2086 # the path list is intentionally split into arguments
references=$(cd "$root" && grep -rn \
    --exclude-dir=target --exclude-dir=.venv --exclude-dir=.git --exclude-dir=node_modules \
    -E 'docs/[A-Za-z0-9_-]+\.md' $public_paths 2>/dev/null || true)
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

# The project notes are private too; a public file must not name them.
# shellcheck disable=SC2086 # the path list is intentionally split into arguments
private_notes=$(cd "$root" && grep -rn --exclude-dir=target --exclude-dir=.venv \
    --exclude-dir=.git --exclude-dir=node_modules -E 'AGENTS\.md' $public_paths 2>/dev/null || true)
if [ -n "$private_notes" ]; then
    while IFS= read -r line; do
        [ -n "$line" ] && note "private project notes referenced from a public file: $line"
    done <<EOF
$private_notes
EOF
fi

# --- 2. Relative Markdown links -----------------------------------------
documents=$(cd "$root" && find . -name '*.md' \
    -not -path './.git/*' -not -path './client/.venv/*' -not -path './server/target/*' \
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
        target=${link%%#*}
        [ -n "$target" ] || continue
        if [ ! -e "$root/$directory/$target" ] && [ ! -e "$root/$target" ]; then
            note "$document: broken link: $link"
        fi
    done <<EOF
$links
EOF
done <<EOF
$documents
EOF

# --- 3. Client version ---------------------------------------------------
version=$(tr -d '[:space:]' <"$root/client/VERSION")
if ! grep -q "^__version__ = \"$version\"$" "$root/client/pull.py"; then
    note "client/VERSION ($version) and pull.py __version__ disagree"
fi

if [ "$fail" -eq 0 ]; then
    printf 'docs-check: clean\n'
fi
exit "$fail"
