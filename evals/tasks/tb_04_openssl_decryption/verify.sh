#!/usr/bin/env bash
set -euo pipefail
# Verify: decrypted content matches original
test -f secret.txt
grep -q "My super secret data" secret.txt
