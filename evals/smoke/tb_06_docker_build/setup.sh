#!/bin/bash
# Setup: provide app.py and a deliberately broken Dockerfile
# The agent must diagnose and fix the Dockerfile
cat > app.py <<'EOF'
print("Hello Docker")
EOF

cat > Dockerfile <<'EOF'
FROM ubuntu:latest
# Missing COPY instruction
# Wrong entrypoint — python not installed, path wrong
ENTRYPOINT ["python3", "app.py"]
EOF
