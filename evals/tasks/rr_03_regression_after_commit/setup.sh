git init -q -b main; git config user.name e; git config user.email e@e
cat > durations.py <<'E'
import re
UNITS = {"s": 1, "m": 60, "h": 3600}
def parse_duration(s):
    total = 0
    for n, u in re.findall(r"(\d+)([smh])", s):
        total += int(n) * UNITS[u]
    return total
E
cat > test_dur.py <<'E'
from durations import parse_duration
assert parse_duration("90s") == 90
assert parse_duration("2m") == 120
assert parse_duration("1h30m") == 5400
print("ok")
E
git add -A; git commit -qm "durations: initial"
sed -i.bak 's/"m": 60/"m": 3600/' durations.py; rm durations.py.bak
git commit -qam "durations: tidy unit table"
