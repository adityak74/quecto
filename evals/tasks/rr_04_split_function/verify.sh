#!/usr/bin/env bash
set -euo pipefail
python3 - <<'E'
from report import summarize, parse_line, aggregate
assert summarize(["alice:3","bob:5","alice:4"])=={"alice":7,"bob":5}
assert parse_line("alice:3")==("alice",3)
assert aggregate([("a",1),("a",2),("b",5)])=={"a":3,"b":5}
E
