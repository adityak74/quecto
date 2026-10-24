mkdir -p data
cat > total.py <<'E'
import csv
print(sum(float(r["amount"]) for r in csv.DictReader(open("data/sales.csv"))))
E
