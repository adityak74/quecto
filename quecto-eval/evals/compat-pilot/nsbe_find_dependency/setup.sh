#!/bin/sh
set -e
cat > requirements.lock <<'EOF'
requests==2.28.1
urllib3==1.26.15
certifi==2022.12.7
EOF
