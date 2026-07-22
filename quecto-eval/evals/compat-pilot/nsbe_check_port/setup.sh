#!/bin/sh
set -e
cat > docker-compose.yml <<'EOF'
services:
  app_server:
    ports:
      - "8080:80"
  cache_server:
    ports:
      - "6379:6379"
  db_server:
    ports:
      - "5432:5432"
EOF
