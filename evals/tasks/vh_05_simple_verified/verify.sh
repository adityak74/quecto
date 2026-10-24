#!/usr/bin/env bash
set -euo pipefail
test "$(cat hello.txt)" = "hello, harness"
head -1 REPORT.md | grep -qx 'STATUS: PASS'
