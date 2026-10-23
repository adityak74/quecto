#!/usr/bin/env bash
set -euo pipefail
python3 - <<'E'
from api import add
assert add(2,3)==5 and add(-1,1)==0
E
head -1 REPORT.md | grep -qx 'STATUS: FAIL'
