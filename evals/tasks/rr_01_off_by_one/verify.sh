#!/usr/bin/env bash
set -euo pipefail
python3 - <<'E'
from lib import moving_avg
assert moving_avg([1,2,3,4],2)==[1.5,2.5,3.5]
assert moving_avg([5],1)==[5.0]
assert moving_avg([1,2],3)==[]
assert moving_avg([1,2,3],3)==[2.0]
E
