#!/bin/sh
set -e
cat > math_utils.py <<'EOF'
def substract(a, b):
    return a - b
EOF
cat > string_utils.py <<'EOF'
def substract_str(a, b):
    return a.replace(b, '')
EOF
