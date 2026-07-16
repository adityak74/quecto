#!/bin/bash

# Setup for Docker build task
echo "FROM python:3.11-slim" > Dockerfile
cat <<'EOF' > app.py
print('Hello Docker')
EOF

echo "COPY app.py /app.py" >> Dockerfile

echo "ENTRYPOINT [\"python\", \"/app.py\"]" >> Dockerfile

# Build the image
docker build -t eval-image .
# Run container and capture output
docker run --rm eval-image > output.txt
