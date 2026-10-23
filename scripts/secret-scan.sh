#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# Secret scan for the repository.
#
# Uses gitleaks when it is installed. Otherwise it falls back to a path and
# content check, and says so on stderr: the fallback is a safety net, not a
# replacement for the real scanner.
#
# Exit codes: 0 clean, 1 a candidate secret was found, 2 a setup error.

set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH='' cd -- "$script_dir/.." && pwd)

found=0
report() {
    printf 'secret-scan: %s\n' "$*" >&2
    found=1
}

if command -v gitleaks >/dev/null 2>&1; then
    printf 'secret-scan: gitleaks\n'
    if ! (cd "$root" && gitleaks detect --no-banner --redact --no-git --source .); then
        printf 'secret-scan: gitleaks reported a candidate secret\n' >&2
        exit 1
    fi
    exit 0
fi

printf 'secret-scan: gitleaks not installed; fallback mode (path and content check)\n'

tracked=$(cd "$root" && git ls-files)
if [ -z "$tracked" ]; then
    printf 'secret-scan: no tracked files\n'
    exit 0
fi

# Paths that must never be tracked. Committed examples are excluded: they are
# documentation, not credentials.
suspect_paths=$(printf '%s\n' "$tracked" | grep -E \
    '(^|/)(auth|lanpull\.clients|lanpull\.access\.json|lanpull\.networks\.json|lanpull\.conf)$|\.(key|pem|p12|pfx)$|(^|/)id_(rsa|dsa|ecdsa|ed25519)$|(^|/)\.env(\.|$)' \
    | grep -Ev '\.example$|^docs/' || true)
if [ -n "$suspect_paths" ]; then
    while IFS= read -r path; do
        [ -n "$path" ] && report "tracked secret-like path: $path"
    done <<EOF
$suspect_paths
EOF
fi

# Content patterns. `git grep` exits 1 when nothing matches, which is the
# expected clean case.
if (cd "$root" && git grep -n -I -E \
    -e '-----BEGIN [A-Z ]*PRIVATE KEY-----' \
    -e 'AKIA[0-9A-Z]{16}' \
    -e 'ghp_[A-Za-z0-9]{36}' \
    -- .) >&2; then
    report 'content pattern matched (see above)'
fi

if [ "$found" -eq 0 ]; then
    printf 'secret-scan: clean (fallback mode)\n'
fi
exit "$found"
