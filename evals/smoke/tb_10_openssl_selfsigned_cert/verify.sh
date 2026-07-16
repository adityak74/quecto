#!/usr/bin/env bash
set -euo pipefail
test -f certs/cert.pem
test -f certs/key.pem
# Subject must contain eval.local
openssl x509 -noout -subject -in certs/cert.pem | grep -q "eval.local"
# Must be valid for at least 364 days
days=$(openssl x509 -noout -dates -in certs/cert.pem | awk -F= '/notAfter/{print $2}' | xargs -I{} python3 -c "from datetime import datetime; import sys; exp=datetime.strptime('{}','%b %d %H:%M:%S %Y %Z'); now=datetime.utcnow(); print((exp-now).days)")
[ "$days" -ge 364 ]
