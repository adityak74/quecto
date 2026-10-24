#!/usr/bin/env bash
set -euo pipefail
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs4").hexdigest()[:8])')
test "$(cat secret.out)" = "unlocked-$code"
test -d work
