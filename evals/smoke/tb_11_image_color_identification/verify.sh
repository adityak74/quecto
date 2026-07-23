#!/usr/bin/env bash
set -euo pipefail
test -f color.txt
grep -qi "red" color.txt
