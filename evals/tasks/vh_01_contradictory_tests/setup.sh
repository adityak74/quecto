cat > calc.py <<'E'
def f(x):
    return x * 2
E
cat > test_calc.py <<'E'
from calc import f
assert f(2) == 4
assert f(2) == 5
print("ok")
E
