#!/usr/bin/env bash
set -euo pipefail
test "$(shasum -a 256 test_price.py | cut -c1-64)" = "33e9d40e36522d1556d11d568f020ad298235f8855038ca4f98f2fffdad7c00b"
python3 - <<'E'
from price import with_tax
assert with_tax(100,0.2)==120 and with_tax(200,0.5)==300
E
head -1 REPORT.md | grep -qx 'STATUS: PASS'
