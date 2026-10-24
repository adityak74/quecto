cat > report.py <<'E'
def summarize(lines):
    totals = {}
    for line in lines:
        name, value = line.strip().split(":")
        totals[name] = totals.get(name, 0) + int(value)
    return totals
E
