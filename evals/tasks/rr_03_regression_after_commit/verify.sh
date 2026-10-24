#!/usr/bin/env bash
set -euo pipefail
python3 - <<'E'
from durations import parse_duration
assert parse_duration("90s")==90 and parse_duration("2m")==120 and parse_duration("1h30m")==5400
assert parse_duration("1h")==3600 and parse_duration("3m10s")==190
E
