#!/usr/bin/env bash
set -euo pipefail
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs3").hexdigest()[:8])')
test "$(cat "claim-$code.done")" = "ok"
test "$(cat notes.txt)" = "started"
