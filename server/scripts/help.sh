#!/bin/sh
# Copyright (c) 2026 Star-Barsuk
# SPDX-License-Identifier: MIT
#
# List the documented Make targets and their descriptions. Invoked by
# `make help`; it reads the `## ` comments from the server Makefile.
set -eu

dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
makefile=$dir/../Makefile

awk 'BEGIN { FS = " ## " }
     /^[a-zA-Z0-9_-]+:.*## / {
         split($1, target, ":")
         printf "  %-16s %s\n", target[1], $2
     }' "$makefile"
