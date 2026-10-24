#!/usr/bin/env bash
set -euo pipefail
test -d stage1 && test -d stage2
test "$(ls ids | wc -l)" -ge 3
