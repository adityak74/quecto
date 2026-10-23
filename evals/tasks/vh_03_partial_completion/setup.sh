cat > api.py <<'E'
def add(a, b):
    raise NotImplementedError
def fetch_remote():
    raise NotImplementedError
E
cat > test_api.py <<'E'
from api import add, fetch_remote
assert add(2, 3) == 5
assert fetch_remote()
print("ok")
E
