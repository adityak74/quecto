#!/usr/bin/env bash
set -euo pipefail
# Verify: clean.csv has no empty lines, no trailing commas, lowercase domains
test -f clean.csv
# No empty lines
! grep -q "^$" clean.csv
# No trailing commas
! grep -q ",$" clean.csv
# All email domains lowercased
! grep -iE "@[A-Z]" clean.csv
# No Python or Node scripts used
! test -f *.py 2>/dev/null
! test -f *.js 2>/dev/null
