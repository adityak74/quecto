#!/usr/bin/env bash
set -euo pipefail
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs1").hexdigest()[:8])')
test "$(cat out/final.txt)" = "$code"
test -f a.txt && test -f b.txt && test -f c.txt
