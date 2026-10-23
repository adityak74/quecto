#!/usr/bin/env bash
set -euo pipefail
code=$(python3 -c 'import hashlib;print(hashlib.sha256(b"quecto-cs5").hexdigest()[:8])')
grep -qx "# Changelog" CHANGELOG.md
grep -qx -- "- ticket T-${code:0:5}" CHANGELOG.md
