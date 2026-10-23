#!/usr/bin/env bash
set -euo pipefail
test ! -s total.txt
head -1 REPORT.md | grep -qx 'STATUS: FAIL'
