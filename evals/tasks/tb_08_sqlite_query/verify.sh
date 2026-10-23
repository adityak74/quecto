#!/usr/bin/env bash
set -euo pipefail
test -f query.py
test -f result.txt
grep -q "Gadget" result.txt
grep -q "49.99" result.txt
