cat > lib.py <<'E'
def moving_avg(xs, k):
    out = []
    for i in range(len(xs) - k):
        out.append(sum(xs[i:i + k]) / k)
    return out
E
cat > test_lib.py <<'E'
from lib import moving_avg
assert moving_avg([1, 2, 3, 4], 2) == [1.5, 2.5, 3.5], moving_avg([1, 2, 3, 4], 2)
print("ok")
E
