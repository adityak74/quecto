#!/bin/sh
set -e
mkdir -p backend frontend
echo '{ "port": 8080 ' > backend/config.json
echo '{ "port": 3000 ' > frontend/config.json
