#!/usr/bin/env bash
set -euo pipefail
ls claim-*.done >/dev/null
test "$(cat notes.txt)" = "started"
