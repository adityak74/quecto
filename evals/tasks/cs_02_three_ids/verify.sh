#!/usr/bin/env bash
set -euo pipefail
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs2").hexdigest()[:8])')
for id in "${code:0:3}" "${code:3:3}" "${code:5:3}"; do test -d "ids/$id"; done
test -d stage1 && test -d stage2
