cat > price.py <<'E'
def with_tax(amount, rate):
    return amount + rate
E
cat > test_price.py <<'E'
from price import with_tax
assert with_tax(100, 0.2) == 120
print("ok")
E
